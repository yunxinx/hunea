use std::{collections::VecDeque, path::Path, sync::Arc};

use conversation_runtime::{ConversationWorker, PreparedTurnOptions, ProviderConversation};
use runtime_domain::{
    context_budget::ContextWindowUsage,
    dynamic_environment::{
        DynamicEnvironmentSessionConfig, enabled_dynamic_environment_sources_for_session_config,
    },
    event_notifier::RuntimeEventNotifier,
    model_catalog::ModelSelection,
    prompt_assembly::PromptAssemblyManagerSnapshot,
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationEvent, ConversationTurnRequest, RuntimePermissionRequest,
        RuntimeRequestMetrics, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityRawValue,
        RuntimeToolActivityStatus, RuntimeToolKind, TranscriptReplayItem, TranscriptUserMessage,
    },
};
use session_store::{SessionHeader, SessionId, SessionPort};
use tool_runtime::{ToolDefinition, ToolExecutorRegistry};

#[cfg(test)]
use super::AgentRuntimeTestHarness;
use super::{
    AgentCommand, AgentCommandReceipt, AgentContextBudgetSnapshot, AgentEvent, AgentEventKind,
    AgentId, AgentRuntime, AgentRuntimeError, AgentRuntimeMount, AgentRuntimePort,
    AgentSessionRestore, AgentTurnId, AgentTurnRequest,
};
use crate::prompt_assembly::{
    AttachedPromptMessageAssembly, ManualSkillPromptUse, PromptAssemblyWorkspace,
};
use crate::runtime::{
    context::{CapabilityLease, RuntimeContext, RuntimeEventStreamCapability},
    dynamic_environment_worker::{
        DynamicEnvironmentInjection, DynamicEnvironmentRequest, DynamicEnvironmentWorker,
        dynamic_environment_snapshot_for_turn,
    },
    llm_port::LlmPort,
    permission_policy::{PermissionPolicy, PermissionTurn},
    prompt_assembly::PromptAssemblySessionSnapshot,
};

struct PendingNativeTurn {
    agent_id: AgentId,
    turn_id: AgentTurnId,
    target: RuntimeTarget,
    provider_request: ConversationTurnRequest,
    transcript_user_message: TranscriptUserMessage,
    manual_skill_activities: Vec<RuntimeToolActivity>,
}

#[derive(Debug, Clone)]
struct ActiveNativeTurn {
    agent_id: AgentId,
    turn_id: AgentTurnId,
    target: RuntimeTarget,
}

/// `NativeAgentRuntime` 封装当前 conversation worker 的完整 turn choreography。
pub struct NativeAgentRuntime {
    worker: ConversationWorker,
    llm_port: LlmPort,
    permission_policy: PermissionPolicy,
    permission_provider_id: String,
    provider_conversation: ProviderConversation,
    dynamic_environment_worker: DynamicEnvironmentWorker,
    event_stream: Option<CapabilityLease<RuntimeEventStreamCapability>>,
    #[cfg(test)]
    test_event_notifier: RuntimeEventNotifier,
    request_policy: RuntimeRequestPolicy,
    loaded_models: conversation_runtime::models::LoadedModelCatalog,
    session_workspace_tools: ToolExecutorRegistry,
    prompt_assembly_tool_definitions: Vec<ToolDefinition>,
    prompt_assembly_manager: Option<PromptAssemblyManagerSnapshot>,
    hunea_config_dir: std::path::PathBuf,
    session_header_template: Option<SessionHeader>,
    prompt_assembly_session_config: Option<DynamicEnvironmentSessionConfig>,
    pending_turn: Option<PendingNativeTurn>,
    active_turn: Option<ActiveNativeTurn>,
    permission_turn: Option<PermissionTurn>,
    pending_events: VecDeque<AgentEvent>,
    manual_skill_activity_sequence: usize,
    is_shutdown: bool,
}

/// Native plugin factory 构造 concrete adapter 后立即擦除 implementation type。
pub(in crate::runtime) fn construct_native_agent_runtime(
    mount: AgentRuntimeMount,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    NativeAgentRuntime::new(mount).map(|runtime| Box::new(runtime) as Box<dyn AgentRuntimePort>)
}

impl NativeAgentRuntime {
    // provider identity 必须与传入的 PermissionPolicy generation 成对传递；将其
    // 隐藏到全局默认值会让 provider replacement 后的 turn 错误地访问旧注册。
    fn new(mount: AgentRuntimeMount) -> Result<Self, String> {
        Self::new_with_notifier(mount, RuntimeEventNotifier::default())
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_for_test(
        options: &crate::runtime::AppRuntimeOptions,
        session_workspace_tools: ToolExecutorRegistry,
        prompt_assembly_tool_definitions: Vec<ToolDefinition>,
        prompt_assembly: PromptAssemblySessionSnapshot,
        session_port: Option<Arc<dyn SessionPort>>,
        event_notifier: RuntimeEventNotifier,
        llm_port: LlmPort,
        permission_policy: PermissionPolicy,
        permission_provider_id: impl Into<String>,
    ) -> Result<Self, String> {
        Self::new_with_notifier(
            AgentRuntimeMount::new(
                options,
                session_workspace_tools,
                prompt_assembly_tool_definitions,
                prompt_assembly,
                session_port,
                llm_port,
                permission_policy,
                permission_provider_id.into(),
            ),
            event_notifier,
        )
    }

    fn new_with_notifier(
        mount: AgentRuntimeMount,
        event_notifier: RuntimeEventNotifier,
    ) -> Result<Self, String> {
        let AgentRuntimeMount {
            loaded_models,
            runtime_request_policy,
            dynamic_environment_observer,
            hunea_config_dir,
            session_header_template,
            session_workspace_tools,
            prompt_assembly_tool_definitions,
            prompt_assembly,
            session_port,
            llm_port,
            permission_policy,
            permission_provider_id,
        } = mount;
        let provider_conversation = fresh_provider_conversation(
            session_port,
            session_header_template.clone(),
            &prompt_assembly,
        )?;
        let event_stream =
            RuntimeContext::event_stream_lease(event_notifier.clone(), "native_agent_bootstrap");
        Ok(Self {
            worker: ConversationWorker::new((*event_stream).clone()),
            llm_port,
            permission_policy,
            permission_provider_id,
            provider_conversation,
            dynamic_environment_worker: DynamicEnvironmentWorker::new(
                dynamic_environment_observer,
                event_stream.clone(),
            ),
            event_stream: None,
            #[cfg(test)]
            test_event_notifier: event_notifier,
            request_policy: runtime_request_policy,
            loaded_models,
            session_workspace_tools,
            prompt_assembly_tool_definitions,
            prompt_assembly_manager: prompt_assembly.manager,
            hunea_config_dir,
            session_header_template,
            prompt_assembly_session_config: prompt_assembly.dynamic_environment_session_config,
            pending_turn: None,
            active_turn: None,
            permission_turn: None,
            pending_events: VecDeque::new(),
            manual_skill_activity_sequence: 0,
            is_shutdown: false,
        })
    }

    pub(crate) fn is_running(&self) -> bool {
        self.worker.is_running()
    }

    pub(crate) fn is_preparing(&self) -> bool {
        self.pending_turn.is_some()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.is_running() || self.is_preparing()
    }

    #[cfg(test)]
    pub(crate) fn has_pending_work(&self) -> bool {
        self.is_busy() || self.dynamic_environment_worker.has_pending_work()
    }

    pub(crate) fn session_id(&self) -> Option<&SessionId> {
        self.provider_conversation.session_id()
    }

    pub(crate) fn is_history_empty(&self) -> bool {
        self.provider_conversation.is_history_empty()
    }

    pub(crate) fn is_idle_empty_session(&self) -> bool {
        !self.is_busy()
            && self.provider_conversation.is_history_empty()
            && self.provider_conversation.session_id().is_none()
    }

    pub(crate) fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, String> {
        if self.is_busy() {
            return Err(
                "Cannot truncate provider conversation while a request is running".to_string(),
            );
        }
        self.provider_conversation
            .truncate_after_user_turns(retained_user_turns)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn context_budget_snapshot(&self) -> AgentContextBudgetSnapshot {
        AgentContextBudgetSnapshot {
            items: self.provider_conversation.context_budget_probe_items(),
            prompt_prelude: self.provider_conversation.prompt_prelude().cloned(),
            upstream_context_tokens: self.provider_conversation.upstream_context_tokens(),
            tool_definitions: crate::runtime::context_budget::context_budget_tool_definitions(
                &self.session_workspace_tools,
            ),
        }
    }

    pub(crate) fn update_empty_session_configuration(
        &mut self,
        prompt_assembly: PromptAssemblySessionSnapshot,
        session_workspace_tools: ToolExecutorRegistry,
    ) {
        self.provider_conversation
            .set_prompt_prelude(prompt_assembly.prompt_prelude);
        self.provider_conversation
            .set_dynamic_environment_session_config(
                prompt_assembly.dynamic_environment_session_config.clone(),
            );
        self.prompt_assembly_manager = prompt_assembly.manager;
        self.prompt_assembly_session_config = prompt_assembly.dynamic_environment_session_config;
        self.session_workspace_tools = session_workspace_tools;
    }

    fn install_restored_conversation(
        &mut self,
        conversation: ProviderConversation,
    ) -> Result<(), String> {
        let worker_result = self.worker.reset_after_clear();
        self.cancel_permission_turn();
        self.dynamic_environment_worker.cancel_pending();
        self.pending_turn = None;
        self.active_turn = None;
        self.pending_events.clear();
        let _ = self.provider_conversation.rollback_pending_user();
        self.permission_policy.clear_context();
        worker_result?;
        self.provider_conversation = conversation;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn permission_context_generation_for_test(&self) -> u64 {
        self.permission_policy.context_generation_for_test()
    }

    #[cfg(test)]
    pub(crate) fn set_worker_cancellation_for_test(
        &mut self,
        cancellation: tokio_util::sync::CancellationToken,
    ) {
        self.worker.cancellation = Some(cancellation);
    }

    #[cfg(test)]
    pub(crate) fn set_pending_turn_for_test(&mut self, request: ConversationTurnRequest) {
        let request = AgentTurnRequest::from_conversation_request(request);
        let (provider_request, transcript_user_message) = request.into_parts();
        self.pending_turn = Some(PendingNativeTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(1),
            target: provider_request.target(),
            provider_request,
            transcript_user_message,
            manual_skill_activities: Vec::new(),
        });
    }

    #[cfg(test)]
    pub(crate) fn dynamic_environment_injection(
        &mut self,
        observer: Arc<dyn crate::dynamic_environment::DynamicEnvironmentObserver>,
    ) -> Result<DynamicEnvironmentInjection, String> {
        let Some(request) = self
            .dynamic_environment_request()
            .map_err(|error| error.to_string())?
        else {
            return Ok(DynamicEnvironmentInjection::default());
        };
        let cancellation = tokio_util::sync::CancellationToken::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(
            crate::runtime::dynamic_environment_worker::build_dynamic_environment_injection(
                observer,
                request,
                &cancellation,
            ),
        )
    }

    fn submit_turn(
        &mut self,
        agent_id: AgentId,
        turn_id: AgentTurnId,
        request: AgentTurnRequest,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        if self.is_shutdown {
            return Err(AgentRuntimeError::Disposed);
        }
        if agent_id != AgentId::MAIN {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        if self.is_busy() {
            return Err(AgentRuntimeError::Busy);
        }

        let target = request.target();
        let activity_label = request.activity_label().to_string();
        let (request, transcript_user_message) = request.into_parts();
        let attached_prompt_assembly =
            self.attached_prompt_message_assembly(&transcript_user_message)?;
        let provider_request = if attached_prompt_assembly.manual_skill_uses.is_empty()
            && attached_prompt_assembly.custom_prompt_uses.is_empty()
        {
            request.clone()
        } else {
            ConversationTurnRequest::new_user_content(
                request.provider_id(),
                request.model_id(),
                transcript_user_message.provider_content_with_text(
                    attached_prompt_assembly.provider_visible_user_text.clone(),
                ),
            )
        };
        let manual_skill_activities =
            self.manual_skill_activities(&attached_prompt_assembly.manual_skill_uses);
        let pending_turn = PendingNativeTurn {
            agent_id,
            turn_id,
            target: target.clone(),
            provider_request,
            transcript_user_message,
            manual_skill_activities,
        };

        if let Some(dynamic_environment_request) = self.dynamic_environment_request()? {
            self.dynamic_environment_worker
                .load(dynamic_environment_request)
                .map_err(AgentRuntimeError::CommandRejected)?;
            self.pending_turn = Some(pending_turn);
        } else {
            let failure_identity = (
                pending_turn.agent_id,
                pending_turn.turn_id,
                pending_turn.target.clone(),
            );
            if let Err(error) =
                self.start_pending_turn(pending_turn, DynamicEnvironmentInjection::default())
            {
                let (agent_id, turn_id, target) = failure_identity;
                self.pending_events.push_back(AgentEvent {
                    agent_id,
                    turn_id,
                    target,
                    kind: AgentEventKind::TurnFailed {
                        message: error.to_string(),
                    },
                });
                self.notify_runtime_event();
            }
        }
        Ok(AgentCommandReceipt::TurnStarted {
            turn_id,
            target,
            activity_label,
        })
    }

    fn interrupt(
        &mut self,
        agent_id: AgentId,
        target: Option<&RuntimeTarget>,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.ensure_agent(agent_id)?;
        if let Some(pending) = self.pending_turn.as_ref() {
            ensure_conversation_target(Some(&pending.target), target)?;
            let target = pending.target.clone();
            self.dynamic_environment_worker.cancel_pending();
            self.pending_turn = None;
            return Ok(AgentCommandReceipt::Interrupted {
                target: Some(target),
            });
        }

        let active_target = self
            .active_turn
            .as_ref()
            .map(|active| active.target.clone());
        ensure_conversation_target(active_target.as_ref(), target)?;
        if self.worker.interrupt() {
            self.cancel_permission_turn();
            Ok(AgentCommandReceipt::Interrupted {
                target: active_target,
            })
        } else {
            Ok(AgentCommandReceipt::Accepted)
        }
    }

    fn respond_permission(
        &mut self,
        agent_id: AgentId,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.ensure_agent(agent_id)?;
        let active_target = self
            .active_turn
            .as_ref()
            .map(|active| active.target.clone());
        ensure_conversation_target(active_target.as_ref(), target)?;
        if active_target.is_none() {
            return Err(AgentRuntimeError::CommandRejected(
                "Conversation worker is not running".to_string(),
            ));
        }
        self.permission_turn
            .as_ref()
            .ok_or_else(|| {
                AgentRuntimeError::CommandRejected(
                    "Conversation permission turn is not active".to_string(),
                )
            })?
            .respond(request_id, option_id)
            .map_err(|error| AgentRuntimeError::CommandRejected(error.to_string()))?;
        Ok(AgentCommandReceipt::Accepted)
    }

    fn notify_runtime_event(&self) {
        if let Some(event_stream) = &self.event_stream {
            event_stream.notify();
        }
        #[cfg(test)]
        if self.event_stream.is_none() {
            self.test_event_notifier.notify();
        }
    }

    fn ensure_agent(&self, agent_id: AgentId) -> Result<(), AgentRuntimeError> {
        if self.is_shutdown {
            return Err(AgentRuntimeError::Disposed);
        }
        if agent_id != AgentId::MAIN {
            return Err(AgentRuntimeError::UnknownAgent);
        }
        Ok(())
    }

    fn start_pending_turn(
        &mut self,
        pending: PendingNativeTurn,
        dynamic_environment: DynamicEnvironmentInjection,
    ) -> Result<(), AgentRuntimeError> {
        let PendingNativeTurn {
            agent_id,
            turn_id,
            target,
            provider_request,
            transcript_user_message,
            manual_skill_activities,
        } = pending;
        let mut turn_options = PreparedTurnOptions::default()
            .with_appended_user_texts(dynamic_environment.appended_user_texts)
            .with_transcript_user_message(transcript_user_message)
            .with_transcript_replay_after_user(
                self.manual_skill_replay_items(&manual_skill_activities),
            );
        if let Some(observations) = dynamic_environment.next_observations {
            turn_options = turn_options.with_dynamic_environment_observations(observations);
        }
        for activity in manual_skill_activities {
            self.pending_events.push_back(AgentEvent {
                agent_id,
                turn_id,
                target: target.clone(),
                kind: AgentEventKind::ToolActivityStarted { activity },
            });
        }
        // provider 解析必须先于 conversation mutation；配置或 client 初始化失败时，turn
        // 必须保持为从未准备或持久化过的状态。
        let provider_lease = self
            .llm_port
            .resolve(
                &ModelSelection::new(provider_request.provider_id(), provider_request.model_id()),
                self.request_policy.timeout(),
            )
            .map_err(|error| AgentRuntimeError::CommandRejected(error.to_string()))?;
        let permission_turn = self
            .permission_policy
            .begin_turn(&self.permission_provider_id)
            .map_err(|error| AgentRuntimeError::CommandRejected(error.to_string()))?;
        let prepared_request = self
            .provider_conversation
            .prepare_turn_with_options(&provider_request, turn_options)
            .map_err(|error| AgentRuntimeError::CommandRejected(error.to_string()))?;

        self.active_turn = Some(ActiveNativeTurn {
            agent_id,
            turn_id,
            target,
        });
        let permission_handler = permission_turn.handler();
        self.permission_turn = Some(permission_turn);
        self.worker.start(
            prepared_request,
            provider_lease,
            self.session_workspace_tools.clone(),
            self.request_policy.clone(),
            Some(permission_handler),
        );
        if !self.pending_events.is_empty() {
            self.notify_runtime_event();
        }
        Ok(())
    }

    fn drain_dynamic_environment(&mut self, events: &mut Vec<AgentEvent>) {
        let Some(result) = self.dynamic_environment_worker.try_recv_injection() else {
            return;
        };
        let Some(pending) = self.pending_turn.take() else {
            return;
        };
        let dynamic_environment = match result {
            Ok(injection) => injection,
            Err(message) => {
                events.push(AgentEvent {
                    agent_id: pending.agent_id,
                    turn_id: pending.turn_id,
                    target: pending.target.clone(),
                    kind: AgentEventKind::PreparationWarning { message },
                });
                DynamicEnvironmentInjection::default()
            }
        };
        let failure_identity = (pending.agent_id, pending.turn_id, pending.target.clone());
        match self.start_pending_turn(pending, dynamic_environment) {
            Ok(()) => events.extend(self.pending_events.drain(..)),
            Err(error) => {
                events.extend(self.pending_events.drain(..));
                let (agent_id, turn_id, target) = failure_identity;
                events.push(AgentEvent {
                    agent_id,
                    turn_id,
                    target,
                    kind: AgentEventKind::TurnFailed {
                        message: error.to_string(),
                    },
                });
            }
        }
    }

    fn drain_worker(&mut self, events: &mut Vec<AgentEvent>) {
        while let Some(active) = self.active_turn.clone() {
            let Some(event) = self.worker.try_recv_event() else {
                self.reconcile_worker_updates();
                break;
            };
            let upstream_context_tokens = if matches!(event, ConversationEvent::Finished { .. }) {
                self.worker.take_upstream_context_tokens()
            } else {
                None
            };
            self.reconcile_worker_updates();
            if upstream_context_tokens.is_some() {
                self.provider_conversation
                    .set_upstream_context_tokens(upstream_context_tokens);
            }
            if event.is_terminal() {
                let _ = self.provider_conversation.rollback_pending_user();
            }
            let kind = self.agent_event_kind(event, &active.target, upstream_context_tokens);
            let is_terminal = kind.is_terminal();
            events.push(AgentEvent {
                agent_id: active.agent_id,
                turn_id: active.turn_id,
                target: active.target,
                kind,
            });
            if is_terminal {
                self.active_turn = None;
                self.cancel_permission_turn();
                break;
            }
        }
    }

    fn agent_event_kind(
        &self,
        event: ConversationEvent,
        target: &RuntimeTarget,
        upstream_context_tokens: Option<usize>,
    ) -> AgentEventKind {
        match event {
            ConversationEvent::SystemMessage { message } => {
                AgentEventKind::SystemMessage { message }
            }
            ConversationEvent::Retrying { message } => AgentEventKind::Retrying { message },
            ConversationEvent::OutputTokenEstimate { total_tokens } => {
                AgentEventKind::OutputTokenEstimate { total_tokens }
            }
            ConversationEvent::InputTokenEstimate { total_tokens } => {
                AgentEventKind::InputTokenEstimate { total_tokens }
            }
            ConversationEvent::Thinking { is_thinking } => AgentEventKind::Thinking { is_thinking },
            ConversationEvent::AssistantDelta { content } => {
                AgentEventKind::AssistantDelta { content }
            }
            ConversationEvent::ReasoningDelta { content } => {
                AgentEventKind::ReasoningDelta { content }
            }
            ConversationEvent::ToolActivityStarted { activity } => {
                AgentEventKind::ToolActivityStarted { activity }
            }
            ConversationEvent::ToolActivityUpdated { update } => {
                AgentEventKind::ToolActivityUpdated { update }
            }
            ConversationEvent::TerminalUpdated { snapshot } => {
                AgentEventKind::TerminalUpdated { snapshot }
            }
            ConversationEvent::Finished { response, metrics } => AgentEventKind::TurnFinished {
                response,
                metrics: metrics.map(|metrics| {
                    RuntimeRequestMetrics::new(
                        metrics.latency,
                        metrics.output_tokens,
                        metrics.duration,
                    )
                }),
                context_usage: self.context_usage(target, upstream_context_tokens),
            },
            ConversationEvent::Failed { message } => AgentEventKind::TurnFailed { message },
            ConversationEvent::Interrupted => AgentEventKind::TurnInterrupted,
        }
    }

    fn context_usage(
        &self,
        target: &RuntimeTarget,
        upstream_context_tokens: Option<usize>,
    ) -> Option<ContextWindowUsage> {
        let used = upstream_context_tokens?;
        let RuntimeTarget::Provider(provider_target) = target;
        let selection = ModelSelection::new(
            provider_target.provider_id.clone(),
            provider_target.model_id.clone(),
        );
        Some(ContextWindowUsage {
            limit: self.loaded_models.context_limit_for(&selection),
            used,
        })
    }

    fn reconcile_worker_updates(&mut self) {
        let session_id = self.worker.take_pending_session_id();
        if let Some(entry_id) = self.worker.take_pending_user_entry_id() {
            let _ = self
                .provider_conversation
                .commit_pending_user(Some(entry_id), session_id);
        } else if let Some(session_id) = session_id {
            self.provider_conversation.set_session_id(session_id);
        }
        let items = self.worker.take_session_items();
        if !items.is_empty() {
            self.provider_conversation.commit_turn_items(items);
        }
    }

    fn drain_permission_requests(&mut self, events: &mut Vec<AgentEvent>) {
        let Some(active) = self.active_turn.clone() else {
            return;
        };
        let Some(turn) = self.permission_turn.as_ref() else {
            return;
        };
        while let Some(request) = turn.try_recv_request() {
            events.push(permission_agent_event(&active, request));
        }
    }

    fn cancel_permission_turn(&mut self) {
        if let Some(mut turn) = self.permission_turn.take() {
            turn.cancel_pending();
        }
    }

    fn dynamic_environment_request(
        &mut self,
    ) -> Result<Option<DynamicEnvironmentRequest>, AgentRuntimeError> {
        let Some(work_dir) = self
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.clone())
        else {
            return Ok(None);
        };
        let session_config = self.resolve_dynamic_environment_session_config(work_dir.as_path());
        let is_first_turn = self.provider_conversation.is_history_empty();
        let Some(snapshot_kind) =
            dynamic_environment_snapshot_for_turn(&session_config, is_first_turn)
        else {
            return Ok(None);
        };
        let sources =
            enabled_dynamic_environment_sources_for_session_config(&session_config, snapshot_kind);
        if sources.is_empty() {
            return Ok(None);
        }
        Ok(Some(DynamicEnvironmentRequest {
            work_dir,
            session_config,
            is_first_turn,
            previous_observations: self
                .provider_conversation
                .dynamic_environment_observations()
                .to_vec(),
        }))
    }

    fn resolve_dynamic_environment_session_config(
        &mut self,
        _work_dir: &Path,
    ) -> DynamicEnvironmentSessionConfig {
        if let Some(config) = self
            .provider_conversation
            .dynamic_environment_session_config()
            .cloned()
        {
            return config;
        }
        let config = self
            .prompt_assembly_session_config
            .clone()
            .unwrap_or_default();
        self.provider_conversation
            .set_dynamic_environment_session_config(Some(config.clone()));
        config
    }

    fn attached_prompt_message_assembly(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly, AgentRuntimeError> {
        let Some(work_dir) = self
            .session_header_template
            .as_ref()
            .map(|header| header.work_dir.as_path())
        else {
            return Ok(AttachedPromptMessageAssembly {
                provider_visible_user_text: user_message.content.clone(),
                manual_skill_uses: Vec::new(),
                custom_prompt_uses: Vec::new(),
            });
        };
        Ok(PromptAssemblyWorkspace::new(
            work_dir,
            &self.hunea_config_dir,
            &self.prompt_assembly_tool_definitions,
        )
        .assemble_attached_prompt_message(self.prompt_assembly_manager.as_ref(), user_message))
    }

    #[cfg(test)]
    pub(crate) fn attached_prompt_message_assembly_for_test(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly, String> {
        self.attached_prompt_message_assembly(user_message)
            .map_err(|error| error.to_string())
    }

    fn manual_skill_activities(
        &mut self,
        uses: &[ManualSkillPromptUse],
    ) -> Vec<RuntimeToolActivity> {
        uses.iter()
            .map(|skill_use| self.synthetic_manual_skill_activity(skill_use))
            .collect()
    }

    fn manual_skill_replay_items(
        &self,
        activities: &[RuntimeToolActivity],
    ) -> Vec<TranscriptReplayItem> {
        activities
            .iter()
            .cloned()
            .map(|activity| TranscriptReplayItem::ToolActivity { activity })
            .collect()
    }

    fn synthetic_manual_skill_activity(
        &mut self,
        skill_use: &ManualSkillPromptUse,
    ) -> RuntimeToolActivity {
        self.manual_skill_activity_sequence = self.manual_skill_activity_sequence.saturating_add(1);
        RuntimeToolActivity {
            activity_id: format!(
                "manual-skill-{}-{}",
                self.manual_skill_activity_sequence, skill_use.skill_name
            ),
            title: format!("Read {}", skill_use.skill_path.display()),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(RuntimeToolActivityRawValue::from(serde_json::json!({
                "path": skill_use.skill_path.display().to_string(),
                "hunea_skill_name": skill_use.skill_name,
                "hunea_skill_origin": skill_use.origin.as_str(),
            }))),
            raw_output: None,
        }
    }
}

impl AgentRuntimePort for NativeAgentRuntime {
    fn activate(
        &mut self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Result<(), String> {
        if self.is_busy() {
            return Err("Cannot replace runtime event stream while Agent is busy".to_string());
        }
        self.worker = ConversationWorker::new((*event_stream).clone());
        self.dynamic_environment_worker
            .rebind_event_stream(event_stream.clone());
        self.event_stream = Some(event_stream);
        self.is_shutdown = false;
        Ok(())
    }

    fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        if self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        self.pending_turn = None;
        self.pending_events.clear();
        self.dynamic_environment_worker.shutdown();
        let worker_result = self
            .worker
            .reset_after_clear()
            .map_err(AgentRuntimeError::Shutdown);
        self.cancel_permission_turn();
        self.active_turn = None;
        self.event_stream = None;
        worker_result
    }

    fn is_busy(&self) -> bool {
        self.is_busy()
    }

    fn session_id(&self) -> Option<SessionId> {
        self.session_id().cloned()
    }

    fn is_history_empty(&self) -> bool {
        self.is_history_empty()
    }

    fn is_idle_empty_session(&self) -> bool {
        self.is_idle_empty_session()
    }

    fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, String> {
        self.truncate_after_user_turns(retained_user_turns)
    }

    fn context_budget_snapshot(&self) -> AgentContextBudgetSnapshot {
        self.context_budget_snapshot()
    }

    fn update_empty_session_configuration(
        &mut self,
        prompt_assembly: PromptAssemblySessionSnapshot,
        session_workspace_tools: ToolExecutorRegistry,
    ) {
        self.update_empty_session_configuration(prompt_assembly, session_workspace_tools);
    }

    fn restore_session(&mut self, restore: AgentSessionRestore) -> Result<(), String> {
        let (session_port, header, session_id, conversation_state) = restore.into_parts();
        let conversation = ProviderConversation::with_resolved_session_port(
            session_port,
            header,
            Some(session_id),
            conversation_state,
        )
        .map_err(|error| error.to_string())?;
        self.install_restored_conversation(conversation)
    }

    #[cfg(test)]
    fn test_harness(&mut self) -> Option<&mut dyn AgentRuntimeTestHarness> {
        Some(self)
    }

    #[cfg(test)]
    fn test_harness_ref(&self) -> Option<&dyn AgentRuntimeTestHarness> {
        Some(self)
    }

    #[cfg(test)]
    fn has_pending_work(&self) -> bool {
        self.has_pending_work()
    }
}

#[cfg(test)]
impl AgentRuntimeTestHarness for NativeAgentRuntime {
    fn append_conversation_items(
        &mut self,
        items: Vec<conversation_runtime::ConversationItem>,
    ) -> Result<(), String> {
        self.provider_conversation
            .append_items(items)
            .map_err(|error| error.to_string())
    }

    fn set_upstream_context_tokens(&mut self, upstream_context_tokens: Option<usize>) {
        self.provider_conversation
            .set_upstream_context_tokens(upstream_context_tokens);
    }

    fn stage_pending_turn(&mut self, request: ConversationTurnRequest) {
        self.set_pending_turn_for_test(request);
    }

    fn set_worker_cancellation(&mut self, cancellation: tokio_util::sync::CancellationToken) {
        self.set_worker_cancellation_for_test(cancellation);
    }

    fn prepare_turn(
        &mut self,
        request: &ConversationTurnRequest,
    ) -> Result<conversation_runtime::PreparedConversationRequest, String> {
        self.provider_conversation
            .prepare_turn(request)
            .map_err(|error| error.to_string())
    }

    fn dynamic_environment_injection(
        &mut self,
        observer: Arc<dyn crate::dynamic_environment::DynamicEnvironmentObserver>,
    ) -> Result<DynamicEnvironmentInjection, String> {
        self.dynamic_environment_injection(observer)
    }

    fn attached_prompt_message_assembly(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly, String> {
        self.attached_prompt_message_assembly_for_test(user_message)
    }
}

impl AgentRuntime for NativeAgentRuntime {
    fn dispatch(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        match command {
            AgentCommand::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } => self.submit_turn(agent_id, turn_id, *request),
            AgentCommand::Interrupt { agent_id, target } => {
                self.interrupt(agent_id, target.as_ref())
            }
            AgentCommand::RespondPermission {
                agent_id,
                target,
                request_id,
                option_id,
            } => self.respond_permission(agent_id, target.as_ref(), &request_id, option_id),
        }
    }

    fn drain_events(&mut self) -> Vec<AgentEvent> {
        if self.is_shutdown {
            return Vec::new();
        }
        let mut events = self.pending_events.drain(..).collect::<Vec<_>>();
        self.drain_dynamic_environment(&mut events);
        self.drain_worker(&mut events);
        self.drain_permission_requests(&mut events);
        events
    }

    fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
        let worker_result = <Self as AgentRuntimePort>::suspend(self);
        self.provider_conversation = ProviderConversation::default();
        self.session_workspace_tools = ToolExecutorRegistry::new();
        self.prompt_assembly_tool_definitions.clear();
        worker_result
    }
}

fn permission_agent_event(
    active: &ActiveNativeTurn,
    request: RuntimePermissionRequest,
) -> AgentEvent {
    AgentEvent {
        agent_id: active.agent_id,
        turn_id: active.turn_id,
        target: active.target.clone(),
        kind: AgentEventKind::PermissionRequested { request },
    }
}

impl Drop for NativeAgentRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn fresh_provider_conversation(
    session_port: Option<Arc<dyn SessionPort>>,
    session_header_template: Option<SessionHeader>,
    prompt_assembly: &PromptAssemblySessionSnapshot,
) -> Result<ProviderConversation, String> {
    let mut provider_conversation = match (session_port, session_header_template) {
        (Some(session_port), Some(header_template)) => {
            ProviderConversation::with_session_port(session_port, header_template)
                .map_err(|error| error.to_string())?
        }
        _ => ProviderConversation::default(),
    };
    provider_conversation.set_prompt_prelude(prompt_assembly.prompt_prelude.clone());
    provider_conversation.set_dynamic_environment_session_config(
        prompt_assembly.dynamic_environment_session_config.clone(),
    );
    Ok(provider_conversation)
}

fn ensure_conversation_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), AgentRuntimeError> {
    match command_target {
        Some(target @ RuntimeTarget::Provider(_)) => match active_target {
            Some(active_target) if active_target == target => Ok(()),
            Some(_) => Err(AgentRuntimeError::CommandRejected(format!(
                "Conversation is not active: {}",
                target.display_label()
            ))),
            None => Err(AgentRuntimeError::CommandRejected(format!(
                "Conversation is not running: {}",
                target.display_label()
            ))),
        },
        None => Ok(()),
    }
}
