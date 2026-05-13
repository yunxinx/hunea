use std::path::PathBuf;

use mo_acp::{AcpSessionCatalog, AcpSessionWorker, build_acp_prompt_from_composer_text};
use mo_core::{
    acp::AcpSessionEvent,
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    request_policy::RuntimeRequestPolicy,
    session::NativeAgentRequest,
    tools::{RuntimeToolExecutorRegistry, builtin::workspace_readonly_tool_registry},
};
use mo_native_agent::{
    ModelProviderRefreshRuntimeState, NativeAgentRuntimeState, models as native_models,
};
use mo_tui::{AcpPromptSubmission, AcpSessionStart, NativeAgentRuntimeEvent, RuntimeDriver};

/// `AppRuntimeOptions` 保存 app 层运行 agent runtime 所需的配置。
#[derive(Debug, Clone, Default)]
pub(crate) struct AppRuntimeOptions {
    pub(crate) acp_sessions: AcpSessionCatalog,
    pub(crate) model_config_path: Option<PathBuf>,
    pub(crate) runtime_request_policy: RuntimeRequestPolicy,
}

/// `AppRuntimeDriver` 负责把 TUI effect 连接到具体 ACP/native runtime。
#[derive(Default)]
pub(crate) struct AppRuntimeDriver {
    options: AppRuntimeOptions,
    acp_worker: Option<AcpSessionWorker>,
    native_agent: NativeAgentRuntimeState,
    model_refresh: ModelProviderRefreshRuntimeState,
}

impl AppRuntimeDriver {
    pub(crate) fn new(options: AppRuntimeOptions) -> Self {
        Self {
            options,
            acp_worker: None,
            native_agent: NativeAgentRuntimeState::default(),
            model_refresh: ModelProviderRefreshRuntimeState::default(),
        }
    }
}

impl RuntimeDriver for AppRuntimeDriver {
    fn drain_acp_events(&mut self) -> Vec<AcpSessionEvent> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Vec::new();
        };

        let mut events = Vec::new();
        while let Some(event) = worker.try_recv_event() {
            events.push(event);
        }
        events
    }

    fn drain_native_agent_events(&mut self) -> Vec<NativeAgentRuntimeEvent> {
        let mut events = Vec::new();
        loop {
            let target = self.native_agent.current_target().cloned();
            let Some(event) = self.native_agent.try_recv_event() else {
                break;
            };
            events.push(NativeAgentRuntimeEvent { target, event });
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
        self.acp_worker.is_some()
            || self.native_agent.is_running()
            || self.model_refresh.is_running()
    }

    fn reset_runtime_session(&mut self) {
        self.native_agent.reset_after_clear();
        self.model_refresh.reset_after_clear();
    }

    fn start_acp_session(&mut self, agent_id: &str) -> Result<AcpSessionStart, String> {
        let Some(command) = self.options.acp_sessions.command(agent_id) else {
            return Err(format!(
                "ACP agent needs installation before starting: {agent_id}"
            ));
        };

        self.acp_worker = Some(AcpSessionWorker::start(command.clone()));
        Ok(AcpSessionStart {
            default_model: command.default_model.clone(),
        })
    }

    fn submit_acp_prompt(&mut self, submission: AcpPromptSubmission) -> Result<(), String> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Err(format!("ACP session is not ready: {}", submission.agent_id));
        };
        if worker.agent_id() != submission.agent_id {
            return Err(format!(
                "ACP session is not active: {}",
                submission.agent_id
            ));
        }

        let prompt = build_acp_prompt_from_composer_text(
            &submission.text,
            &submission.current_dir,
            submission.identity.as_ref(),
        );
        worker
            .send_prompt(prompt)
            .map_err(|error| error.to_string())
    }

    fn respond_acp_permission(
        &mut self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker
            .respond_permission(request_id, option_id)
            .map_err(|error| error.to_string())
    }

    fn set_acp_model(&mut self, config_id: Option<String>, value: String) -> Result<(), String> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker
            .set_model(config_id, value)
            .map_err(|error| error.to_string())
    }

    fn stop_acp_background_terminals(&mut self) -> Result<(), String> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker
            .stop_background_terminals()
            .map_err(|error| error.to_string())
    }

    fn cancel_acp_prompt(&mut self) -> Result<(), String> {
        let Some(worker) = self.acp_worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker.cancel_prompt().map_err(|error| error.to_string())
    }

    fn send_native_agent(&mut self, request: NativeAgentRequest) -> Result<String, String> {
        if self.native_agent.is_running() {
            return Err("Chat request is already running".to_string());
        }

        let activity_label = request.llm_request().model_id.clone();
        let tools = native_agent_workspace_tools();
        let request = request.with_tools(tools.definitions());
        self.native_agent
            .start(request, tools, self.options.runtime_request_policy.clone());
        Ok(activity_label)
    }

    fn interrupt_native_agent(&mut self) -> bool {
        self.native_agent.interrupt()
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

fn native_agent_workspace_tools() -> RuntimeToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_readonly_tool_registry(root)
}
