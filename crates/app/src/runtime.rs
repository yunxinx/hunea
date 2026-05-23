use std::path::PathBuf;

use mo_core::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    request_policy::RuntimeRequestPolicy,
    session::{
        NativeAgentEvent, NativeAgentTurnRequest, RuntimeCommand, RuntimeCommandReceipt,
        RuntimeEvent, RuntimeRequestMetrics, RuntimeTarget,
    },
};
use mo_native_agent::{
    ModelProviderRefreshRuntimeState, NativeAgentRuntimeState, NativeAgentSession,
    models as native_models,
};
use mo_tools::{ToolExecutorRegistry, builtin::workspace_tool_registry};
use mo_tui::RuntimeCoordinator;

/// `AppRuntimeOptions` 保存 app 层运行 agent runtime 所需的配置。
#[derive(Debug, Clone, Default)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) model_config_path: Option<PathBuf>,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
}

/// `AppRuntimeCoordinator` 负责把 TUI runtime command 连接到 native runtime。
#[derive(Default)]
pub(crate) struct AppRuntimeCoordinator {
    options: AppRuntimeOptions,
    native_agent: NativeAgentRuntimeState,
    native_session: NativeAgentSession,
    model_refresh: ModelProviderRefreshRuntimeState,
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(options: AppRuntimeOptions) -> Self {
        Self {
            options,
            native_agent: NativeAgentRuntimeState::default(),
            native_session: NativeAgentSession::default(),
            model_refresh: ModelProviderRefreshRuntimeState::default(),
        }
    }

    fn handle_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::SubmitNativeAgent { target, request } => {
                self.start_native_agent(target, request)
            }
            RuntimeCommand::TruncateNativeAgentSession {
                retained_user_turns,
            } => {
                if self.native_agent.is_running() {
                    return Err(
                        "Cannot truncate native agent session while a request is running"
                            .to_string(),
                    );
                }
                self.native_session
                    .truncate_after_user_turns(retained_user_turns);
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::Interrupt { target } => self.interrupt_runtime(target),
            RuntimeCommand::RespondPermission {
                target,
                request_id,
                option_id,
                ..
            } => {
                self.respond_permission(target.as_ref(), &request_id, option_id)?;
                Ok(RuntimeCommandReceipt::Accepted)
            }
            RuntimeCommand::Reset => {
                self.native_agent.reset_after_clear();
                self.native_session.clear();
                self.model_refresh.reset_after_clear();
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn respond_native_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        ensure_native_command_target(self.native_agent.current_target(), target)?;
        self.native_agent.respond_permission(request_id, option_id)
    }

    fn respond_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        match target {
            Some(RuntimeTarget::NativeAgent(_)) => {
                self.respond_native_permission(target, request_id, option_id)
            }
            None if self.native_agent.is_running() => {
                self.respond_native_permission(None, request_id, option_id)
            }
            None => Err("Native agent is not running".to_string()),
        }
    }

    fn start_native_agent(
        &mut self,
        target: RuntimeTarget,
        request: NativeAgentTurnRequest,
    ) -> Result<RuntimeCommandReceipt, String> {
        let request_target = request.target();
        if target != request_target {
            return Err(format!(
                "Native agent command target does not match request: {}",
                target.display_label()
            ));
        }
        if self.native_agent.is_running() {
            return Err("Chat request is already running".to_string());
        }

        let activity_label = request.model_id().to_string();
        let execution_request = self
            .native_session
            .prepare_turn(&request)
            .map_err(|error| error.to_string())?;
        let tools = native_agent_workspace_tools();
        self.native_agent.start(
            execution_request,
            tools,
            self.options.runtime_request_policy.clone(),
        );
        Ok(RuntimeCommandReceipt::NativeAgentStarted { activity_label })
    }

    fn interrupt_runtime(
        &mut self,
        target: Option<RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        match target {
            Some(target @ RuntimeTarget::NativeAgent(_)) => {
                self.interrupt_native_agent(Some(&target))
            }
            None => {
                if self.native_agent.is_running() {
                    return self.interrupt_native_agent(None);
                }
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn interrupt_native_agent(
        &mut self,
        command_target: Option<&RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        let active_target = self.native_agent.current_target().cloned();
        ensure_native_command_target(active_target.as_ref(), command_target)?;
        if self.native_agent.interrupt() {
            Ok(RuntimeCommandReceipt::Interrupted {
                target: active_target,
            })
        } else {
            Ok(RuntimeCommandReceipt::Accepted)
        }
    }
}

impl RuntimeCoordinator for AppRuntimeCoordinator {
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        loop {
            let target = self.native_agent.current_target().cloned();
            let Some(event) = self.native_agent.try_recv_event() else {
                self.reconcile_native_session_updates();
                break;
            };
            self.reconcile_native_session_updates();
            if event.is_terminal() {
                self.native_session.rollback_pending_user();
            }
            events.push(runtime_event_from_native_agent_event(target, event));
        }
        events
    }

    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.model_refresh.try_recv_event() {
            events.push(event);
        }
        events
    }

    fn has_background_runtime(&self) -> bool {
        self.native_agent.is_running() || self.model_refresh.is_running()
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.handle_runtime_command(command)
    }

    fn persist_selected_model(&mut self, selection: &ModelSelection) -> Result<(), String> {
        native_models::write_default_model(self.options.model_config_path.as_deref(), selection)
            .map(|_| ())
            .map_err(|error| format!("Failed to save default model: {error}"))
    }

    fn refresh_model_provider(&mut self, request: ProviderSyncRequest) -> Result<(), String> {
        if self.model_refresh.is_running() {
            return Err("Model refresh is already running".to_string());
        }

        self.model_refresh.start(request);
        Ok(())
    }
}

impl AppRuntimeCoordinator {
    fn reconcile_native_session_updates(&mut self) {
        if self.native_agent.take_provider_turn_started() {
            self.native_session.commit_pending_user();
        }

        let messages = self.native_agent.take_session_messages();
        if messages.is_empty() {
            return;
        }

        self.native_session.commit_pending_user();
        self.native_session.commit_turn_messages(messages);
    }
}

fn native_agent_workspace_tools() -> ToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_tool_registry(root)
}

fn runtime_event_from_native_agent_event(
    target: Option<RuntimeTarget>,
    event: NativeAgentEvent,
) -> RuntimeEvent {
    match event {
        NativeAgentEvent::Retrying { message } => RuntimeEvent::Retrying { target, message },
        NativeAgentEvent::OutputTokenEstimate { total_tokens } => {
            RuntimeEvent::OutputTokenEstimate {
                target,
                total_tokens,
            }
        }
        NativeAgentEvent::InputTokenEstimate { total_tokens } => RuntimeEvent::InputTokenEstimate {
            target,
            total_tokens,
        },
        NativeAgentEvent::Thinking { is_thinking } => RuntimeEvent::Thinking {
            target,
            is_thinking,
        },
        NativeAgentEvent::AssistantDelta { content } => RuntimeEvent::AssistantDelta {
            target: target.expect("native agent target should be available for assistant delta"),
            content,
        },
        NativeAgentEvent::ReasoningDelta { content } => RuntimeEvent::ReasoningDelta {
            target: target.expect("native agent target should be available for reasoning delta"),
            content,
        },
        NativeAgentEvent::ToolActivityStarted { activity } => RuntimeEvent::ToolActivityStarted {
            target: target.expect("native agent target should be available for tool activity"),
            activity,
        },
        NativeAgentEvent::ToolActivityUpdated { update } => RuntimeEvent::ToolActivityUpdated {
            target: target.expect("native agent target should be available for tool activity"),
            update,
        },
        NativeAgentEvent::PermissionRequested { request } => RuntimeEvent::PermissionRequested {
            target: target.expect("native agent target should be available for permission request"),
            request,
        },
        NativeAgentEvent::Finished { response, metrics } => RuntimeEvent::MessageFinished {
            target,
            content: response.content,
            reasoning_content: response.reasoning_content,
            reasoning_duration: response.reasoning_duration,
            finish_reason: None,
            metrics: metrics.map(|metrics| {
                RuntimeRequestMetrics::new(metrics.latency, metrics.output_tokens, metrics.duration)
            }),
        },
        NativeAgentEvent::Failed { message } => RuntimeEvent::Failed { target, message },
        NativeAgentEvent::Interrupted => RuntimeEvent::Interrupted { target },
    }
}

fn ensure_native_command_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), String> {
    match command_target {
        Some(target @ RuntimeTarget::NativeAgent(_)) => match active_target {
            Some(active_target) if active_target == target => Ok(()),
            Some(_) => Err(format!(
                "Native agent is not active: {}",
                target.display_label()
            )),
            None => Err(format!(
                "Native agent is not running: {}",
                target.display_label()
            )),
        },
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::{AppRuntimeCoordinator, AppRuntimeOptions, ensure_native_command_target};
    use mo_core::{
        provider::ProviderKind,
        session::{
            ChatMessage, NativeAgentTurnRequest, RuntimeCommand, RuntimeEvent, RuntimeTarget,
        },
    };
    use mo_tui::RuntimeCoordinator;

    #[test]
    fn native_command_target_must_match_running_agent() {
        let active_target = RuntimeTarget::native_agent("openai", "gpt-4o-mini");
        assert!(ensure_native_command_target(Some(&active_target), None).is_ok());
        assert!(ensure_native_command_target(Some(&active_target), Some(&active_target)).is_ok());

        let inactive_target = RuntimeTarget::native_agent("openai", "gpt-4.1-mini");
        let inactive_error =
            ensure_native_command_target(Some(&active_target), Some(&inactive_target))
                .expect_err("wrong native target should be rejected");
        assert!(inactive_error.contains("Native agent is not active"));

        let stopped_error = ensure_native_command_target(None, Some(&active_target))
            .expect_err("explicit native target should require a running agent");
        assert!(stopped_error.contains("Native agent is not running"));
    }

    #[test]
    fn native_agent_failure_before_provider_request_rolls_back_pending_user() {
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            runtime_request_policy: mo_core::request_policy::RuntimeRequestPolicy::new(
                0,
                Vec::new(),
                1,
            ),
            ..AppRuntimeOptions::default()
        });
        let request = NativeAgentTurnRequest::new(
            "openai",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            None,
            None,
            ChatMessage::user("hello".to_string()),
        );
        let target = request.target();

        coordinator
            .handle_runtime_command(RuntimeCommand::SubmitNativeAgent { target, request })
            .expect("native request should start");

        let mut events = Vec::new();
        for _ in 0..50 {
            events.extend(RuntimeCoordinator::drain_runtime_events(&mut coordinator));
            if events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::Failed { .. }))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::Failed { .. })),
            "preflight failure should be reported"
        );
        assert!(coordinator.native_session.history().is_empty());

        let next_request = NativeAgentTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ChatMessage::user("next".to_string()),
        );
        coordinator
            .native_session
            .prepare_turn(&next_request)
            .expect("failed preflight turn should not leave stale pending state");
    }
}
