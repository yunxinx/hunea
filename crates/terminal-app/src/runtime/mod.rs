mod event_mapping;
mod managed_search_authorization;

use std::{path::PathBuf, sync::Arc};

use conversation_runtime::{
    ConversationWorker, ModelRefreshWorker, ProviderConversation, models as provider_models,
};
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationEvent, ConversationTurnRequest, RuntimeCommand, RuntimeCommandReceipt,
        RuntimeEvent, RuntimeTarget,
    },
};
use session_store::{SessionHeader, SessionStore};
use terminal_ui::RuntimeCoordinator;
use tool_runtime::{ToolExecutorRegistry, builtin::ManagedSearchToolConfig};

use self::{
    event_mapping::{
        runtime_event_from_conversation_event, should_defer_runtime_event_for_render_barrier,
    },
    managed_search_authorization::{
        conversation_workspace_tools, persist_managed_search_tool_authorization,
    },
};

/// `AppRuntimeOptions` 保存 app 层对话运行时所需的配置。
#[derive(Clone, Default)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) model_config_path: Option<PathBuf>,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
    pub(crate) managed_search_tools: ManagedSearchToolConfig,
    pub(crate) managed_search_authorization_config_path: Option<PathBuf>,
    pub(crate) session_store: Option<Arc<dyn SessionStore>>,
    pub(crate) session_header_template: Option<SessionHeader>,
}

/// `AppRuntimeCoordinator` 负责把 TUI runtime command 连接到对话运行时。
pub(crate) struct AppRuntimeCoordinator {
    options: AppRuntimeOptions,
    conversation_worker: ConversationWorker,
    provider_conversation: ProviderConversation,
    model_refresh: ModelRefreshWorker,
    workspace_tools: ToolExecutorRegistry,
    pending_runtime_events: Vec<RuntimeEvent>,
}

impl AppRuntimeCoordinator {
    pub(crate) fn new(options: AppRuntimeOptions) -> Self {
        let workspace_tools = conversation_workspace_tools(&options.managed_search_tools);
        let provider_conversation = match (
            options.session_store.clone(),
            options.session_header_template.clone(),
        ) {
            (Some(store), Some(header_template)) => {
                ProviderConversation::with_session_store(store, header_template, None)
                    .expect("session store should initialize without a session id")
            }
            _ => ProviderConversation::default(),
        };
        Self {
            options,
            conversation_worker: ConversationWorker::default(),
            provider_conversation,
            model_refresh: ModelRefreshWorker::default(),
            workspace_tools,
            pending_runtime_events: Vec::new(),
        }
    }

    fn handle_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::SubmitConversationTurn { target, request } => {
                self.start_conversation_turn(target, request)
            }
            RuntimeCommand::TruncateConversation {
                retained_user_turns,
            } => {
                if self.conversation_worker.is_running() {
                    return Err(
                        "Cannot truncate provider conversation while a request is running"
                            .to_string(),
                    );
                }
                self.provider_conversation
                    .truncate_after_user_turns(retained_user_turns)
                    .map_err(|error| error.to_string())?;
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
                self.conversation_worker.reset_after_clear();
                self.provider_conversation.clear();
                self.model_refresh.reset_after_clear();
                self.workspace_tools =
                    conversation_workspace_tools(&self.options.managed_search_tools);
                self.pending_runtime_events.clear();
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn respond_conversation_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        ensure_conversation_target(self.conversation_worker.current_target(), target)?;
        self.conversation_worker
            .respond_permission(request_id, option_id)
    }

    fn respond_permission(
        &mut self,
        target: Option<&RuntimeTarget>,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        match target {
            Some(RuntimeTarget::Provider(_)) => {
                self.respond_conversation_permission(target, request_id, option_id)
            }
            None if self.conversation_worker.is_running() => {
                self.respond_conversation_permission(None, request_id, option_id)
            }
            None => Err("Conversation worker is not running".to_string()),
        }
    }

    fn start_conversation_turn(
        &mut self,
        target: RuntimeTarget,
        request: ConversationTurnRequest,
    ) -> Result<RuntimeCommandReceipt, String> {
        let request_target = request.target();
        if target != request_target {
            return Err(format!(
                "Conversation target does not match request: {}",
                target.display_label()
            ));
        }
        if self.conversation_worker.is_running() {
            return Err("Conversation request is already running".to_string());
        }

        let activity_label = request.model_id().to_string();
        let prepared_request = self
            .provider_conversation
            .prepare_turn(&request)
            .map_err(|error| error.to_string())?;
        self.conversation_worker.start(
            prepared_request,
            self.workspace_tools.clone(),
            self.options.runtime_request_policy.clone(),
        );
        Ok(RuntimeCommandReceipt::ConversationStarted { activity_label })
    }

    fn interrupt_runtime(
        &mut self,
        target: Option<RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        match target {
            Some(target @ RuntimeTarget::Provider(_)) => {
                self.interrupt_conversation_worker(Some(&target))
            }
            None => {
                if self.conversation_worker.is_running() {
                    return self.interrupt_conversation_worker(None);
                }
                Ok(RuntimeCommandReceipt::Accepted)
            }
        }
    }

    fn interrupt_conversation_worker(
        &mut self,
        command_target: Option<&RuntimeTarget>,
    ) -> Result<RuntimeCommandReceipt, String> {
        let active_target = self.conversation_worker.current_target().cloned();
        ensure_conversation_target(active_target.as_ref(), command_target)?;
        if self.conversation_worker.interrupt() {
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
        if !self.pending_runtime_events.is_empty() {
            return std::mem::take(&mut self.pending_runtime_events);
        }

        let mut events = Vec::new();
        loop {
            let target = self.conversation_worker.current_target().cloned();
            let Some(event) = self.conversation_worker.try_recv_event() else {
                self.reconcile_conversation_updates();
                break;
            };
            self.reconcile_conversation_updates();
            if let ConversationEvent::ManagedSearchToolAuthorization { tool } = event {
                if let Some(event) = persist_managed_search_tool_authorization(
                    &mut self.options,
                    &mut self.workspace_tools,
                    tool,
                    target.clone(),
                ) {
                    events.push(event);
                }
                continue;
            }
            if event.is_terminal() {
                self.provider_conversation.rollback_pending_user();
            }
            let runtime_event = runtime_event_from_conversation_event(target, event);
            if should_defer_runtime_event_for_render_barrier(&events, &runtime_event) {
                self.pending_runtime_events.push(runtime_event);
                break;
            }
            events.push(runtime_event);
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
        self.conversation_worker.is_running() || self.model_refresh.is_running()
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.handle_runtime_command(command)
    }

    fn persist_selected_model(&mut self, selection: &ModelSelection) -> Result<(), String> {
        provider_models::write_default_model(self.options.model_config_path.as_deref(), selection)
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
    fn reconcile_conversation_updates(&mut self) {
        if let Some(entry_id) = self.conversation_worker.take_pending_user_entry_id() {
            self.provider_conversation
                .commit_pending_user(Some(entry_id));
        }

        let items = self.conversation_worker.take_session_items();
        if items.is_empty() {
            return;
        }

        self.provider_conversation.commit_turn_items(items);
    }

    #[cfg(test)]
    fn persist_managed_search_tool_authorization(
        &mut self,
        tool: runtime_domain::session::ManagedSearchTool,
        target: Option<RuntimeTarget>,
    ) -> Option<RuntimeEvent> {
        persist_managed_search_tool_authorization(
            &mut self.options,
            &mut self.workspace_tools,
            tool,
            target,
        )
    }
}

fn ensure_conversation_target(
    active_target: Option<&RuntimeTarget>,
    command_target: Option<&RuntimeTarget>,
) -> Result<(), String> {
    match command_target {
        Some(target @ RuntimeTarget::Provider(_)) => match active_target {
            Some(active_target) if active_target == target => Ok(()),
            Some(_) => Err(format!(
                "Conversation is not active: {}",
                target.display_label()
            )),
            None => Err(format!(
                "Conversation is not running: {}",
                target.display_label()
            )),
        },
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        AppRuntimeCoordinator, AppRuntimeOptions, ensure_conversation_target,
        should_defer_runtime_event_for_render_barrier,
    };
    use provider_protocol::{ConversationItem, Role};
    use runtime_domain::{
        provider::ProviderKind,
        session::{
            ConversationTurnRequest, ManagedSearchTool, RuntimeCommand, RuntimeEvent,
            RuntimePermissionRequest, RuntimeTarget,
        },
    };
    use terminal_ui::RuntimeCoordinator;

    #[test]
    fn conversation_target_must_match_running_worker() {
        let active_target = RuntimeTarget::provider("openai", "gpt-4o-mini");
        assert!(ensure_conversation_target(Some(&active_target), None).is_ok());
        assert!(ensure_conversation_target(Some(&active_target), Some(&active_target)).is_ok());

        let inactive_target = RuntimeTarget::provider("openai", "gpt-4.1-mini");
        let inactive_error =
            ensure_conversation_target(Some(&active_target), Some(&inactive_target))
                .expect_err("wrong conversation target should be rejected");
        assert!(inactive_error.contains("Conversation is not active"));

        let stopped_error = ensure_conversation_target(None, Some(&active_target))
            .expect_err("explicit conversation target should require a running worker");
        assert!(stopped_error.contains("Conversation is not running"));
    }

    #[test]
    fn token_estimate_creates_render_barrier_before_permission_request() {
        let output_batch = vec![RuntimeEvent::OutputTokenEstimate {
            target: Some(RuntimeTarget::provider("local", "qwen3")),
            total_tokens: 57,
        }];
        let input_batch = vec![RuntimeEvent::InputTokenEstimate {
            target: Some(RuntimeTarget::provider("local", "qwen3")),
            total_tokens: 12,
        }];
        let permission_event = RuntimeEvent::PermissionRequested {
            target: RuntimeTarget::provider("local", "qwen3"),
            request: RuntimePermissionRequest::new(
                "permission-1",
                Some("Write temp.md".into()),
                vec![],
            ),
        };

        assert!(
            should_defer_runtime_event_for_render_barrier(&output_batch, &permission_event),
            "permission should wait for the output token estimate batch to render first"
        );
        assert!(
            should_defer_runtime_event_for_render_barrier(&input_batch, &permission_event),
            "permission should wait for the input token estimate batch to render first"
        );
        assert!(
            !should_defer_runtime_event_for_render_barrier(&[], &permission_event),
            "permission should not be deferred when there is no token estimate to render"
        );
    }

    #[test]
    fn app_layer_persists_managed_search_tool_authorization() {
        let root = temp_test_dir("managed-search-authorization");
        let config_path = root.join("config.toml");
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            managed_search_authorization_config_path: Some(config_path.clone()),
            ..AppRuntimeOptions::default()
        });

        let event =
            coordinator.persist_managed_search_tool_authorization(ManagedSearchTool::Fd, None);

        assert_eq!(event, None);
        assert_eq!(
            coordinator.options.managed_search_tools.allow_managed_fd,
            Some(true)
        );
        let content = fs::read_to_string(&config_path).expect("config should be readable");
        assert!(content.contains("allow_managed_fd = true"));
        cleanup(&root);
    }

    #[test]
    fn conversation_failure_before_provider_request_rolls_back_pending_user() {
        let mut coordinator = AppRuntimeCoordinator::new(AppRuntimeOptions {
            runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
                0,
                Vec::new(),
                1,
            ),
            ..AppRuntimeOptions::default()
        });
        let request = ConversationTurnRequest::new(
            "openai",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        );
        let target = request.target();

        coordinator
            .handle_runtime_command(RuntimeCommand::SubmitConversationTurn { target, request })
            .expect("conversation request should start");

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
        assert!(coordinator.provider_conversation.history().is_empty());

        let next_request = ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "next"),
        );
        coordinator
            .provider_conversation
            .prepare_turn(&next_request)
            .expect("failed preflight turn should not leave stale pending state");
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
