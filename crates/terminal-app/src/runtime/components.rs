use std::sync::Arc;

use conversation_runtime::{ModelRefreshWorker, RuntimeEventNotifier};
use tool_runtime::{ToolDefinition, ToolExecutorRegistry};

use super::{
    AppRuntimeOptions,
    agent::{AgentRuntime, NativeAgentRuntime},
    context_budget_worker::ContextBudgetWorker,
    lifecycle::{CapabilityKey, ComponentDefinition, ComponentGraph, EffectId, EffectScope},
    session_tools_for_manager,
    session_worker::SessionStoreWorker,
    tool_definitions_from_registry,
    workspace_tools::conversation_workspace_tools,
};
use terminal_ui::RuntimeWake;

/// `RuntimeComponents` 是 coordinator 的长期 runtime owner。
///
/// 它把 native Agent adapter、host workers、tool view、notifier 和 lifecycle graph 放在
/// 同一所有权边界，让 reset/shutdown 不再依赖 coordinator 手工枚举底层 conversation
/// resources。
pub(super) struct RuntimeComponents {
    pub(super) agent_runtime: NativeAgentRuntime,
    pub(super) model_refresh: ModelRefreshWorker,
    pub(super) workspace_tools: ToolExecutorRegistry,
    pub(super) session_workspace_tools: ToolExecutorRegistry,
    pub(super) prompt_assembly_tool_definitions: Vec<ToolDefinition>,
    pub(super) session_store_worker: SessionStoreWorker,
    pub(super) context_budget_worker: ContextBudgetWorker,
    pub(super) runtime_event_notifier: RuntimeEventNotifier,
    effect_scope: EffectScope,
    runtime_wake_effect: Option<EffectId>,
    pub(super) lifecycle: ComponentGraph,
    is_shutdown: bool,
}

impl RuntimeComponents {
    pub(super) fn new(options: &AppRuntimeOptions) -> Result<Self, String> {
        let workspace_tools =
            conversation_workspace_tools(&options.managed_ripgrep, &options.hunea_config_dir);
        let prompt_assembly_tool_definitions = tool_definitions_from_registry(&workspace_tools);
        let session_workspace_tools =
            session_tools_for_manager(&workspace_tools, options.prompt_assembly_manager.as_ref());
        let runtime_event_notifier = RuntimeEventNotifier::default();
        let mut lifecycle = ComponentGraph::default();
        for capability in [
            "model_catalog",
            "prompt_assembly",
            "runtime_event_stream",
            "tool_catalog",
        ] {
            lifecycle.add_capability(capability);
        }
        if options.session_store.is_some() {
            lifecycle.add_capability("session_persistence");
        }
        lifecycle.declare(
            ComponentDefinition::new("native_agent_runtime")
                .requires("model_catalog")
                .requires("prompt_assembly")
                .requires("tool_catalog")
                .observes("session_persistence"),
        );
        lifecycle.declare(
            ComponentDefinition::new("prompt_assembly")
                .requires("tool_catalog")
                .observes("session_persistence"),
        );
        lifecycle.declare(
            ComponentDefinition::new("ui_runtime_bridge")
                .requires("runtime_event_stream")
                .requires("runtime_wake"),
        );
        lifecycle.take_transitions();

        let agent_runtime = NativeAgentRuntime::new(
            options,
            session_workspace_tools.clone(),
            prompt_assembly_tool_definitions.clone(),
            runtime_event_notifier.clone(),
        )?;
        Ok(Self {
            agent_runtime,
            model_refresh: ModelRefreshWorker::new(runtime_event_notifier.clone()),
            workspace_tools,
            session_workspace_tools,
            prompt_assembly_tool_definitions,
            session_store_worker: SessionStoreWorker::new(runtime_event_notifier.clone()),
            context_budget_worker: ContextBudgetWorker::new(runtime_event_notifier.clone())
                .map_err(|error| error.to_string())?,
            runtime_event_notifier,
            effect_scope: EffectScope::default(),
            runtime_wake_effect: None,
            lifecycle,
            is_shutdown: false,
        })
    }

    pub(super) fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        self.remove_runtime_wake()?;
        let mut binding = self
            .runtime_event_notifier
            .bind_callback(move || wake.wake());
        let effect_id = self
            .effect_scope
            .register("runtime-wake", move || {
                binding.dispose();
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        self.runtime_wake_effect = Some(effect_id);
        self.lifecycle.add_capability("runtime_wake");
        self.discard_lifecycle_transitions();
        Ok(())
    }

    pub(super) fn remove_runtime_wake(&mut self) -> Result<(), String> {
        self.lifecycle
            .remove_capability(&CapabilityKey::from("runtime_wake"));
        self.discard_lifecycle_transitions();
        if let Some(effect_id) = self.runtime_wake_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn reset_after_clear(&mut self, options: &AppRuntimeOptions) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let workspace_tools =
            conversation_workspace_tools(&options.managed_ripgrep, &options.hunea_config_dir);
        let prompt_assembly_tool_definitions = tool_definitions_from_registry(&workspace_tools);
        let session_workspace_tools =
            session_tools_for_manager(&workspace_tools, options.prompt_assembly_manager.as_ref());
        let replaced = [
            CapabilityKey::from("model_catalog"),
            CapabilityKey::from("prompt_assembly"),
            CapabilityKey::from("tool_catalog"),
        ];
        for key in &replaced {
            self.lifecycle.remove_capability(key);
        }
        self.discard_lifecycle_transitions();

        let mut failures = Vec::new();
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        if let Err(error) = self.model_refresh.reset_after_clear() {
            failures.push(error);
        }
        self.context_budget_worker.cancel_pending();
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }

        // 旧 adapter 完全 quiescent 后才创建新 generation，避免 reset 期间存在两个
        // native worker path；构造失败时 capability 仍保持 removed，不发布半成品。
        let fresh_agent_runtime = NativeAgentRuntime::new(
            options,
            session_workspace_tools.clone(),
            prompt_assembly_tool_definitions.clone(),
            self.runtime_event_notifier.clone(),
        )?;
        self.agent_runtime = fresh_agent_runtime;
        self.workspace_tools = workspace_tools;
        self.prompt_assembly_tool_definitions = prompt_assembly_tool_definitions;
        self.session_workspace_tools = session_workspace_tools;
        for key in replaced {
            self.lifecycle.replace_capability(&key);
        }
        self.discard_lifecycle_transitions();
        Ok(())
    }

    pub(super) fn shutdown(
        &mut self,
        session_store: Option<&Arc<dyn session_store::SessionStore>>,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        let mut failures = Vec::new();
        if let Err(error) = self.remove_runtime_wake() {
            failures.push(error);
        }
        if let Some(error) = self.effect_scope.dispose().error_message() {
            failures.push(error);
        }
        for key in [
            "model_catalog",
            "prompt_assembly",
            "runtime_event_stream",
            "tool_catalog",
            "session_persistence",
        ] {
            self.lifecycle.remove_capability(&CapabilityKey::from(key));
        }
        self.discard_lifecycle_transitions();
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        if let Err(error) = self.context_budget_worker.shutdown() {
            failures.push(error);
        }
        if let Err(error) = self.model_refresh.shutdown() {
            failures.push(error);
        }
        if self.session_store_worker.is_running()
            && let Some(store) = session_store
            && let Err(error) = self.session_store_worker.flush_all(Arc::clone(store))
        {
            failures.push(error);
        }
        if let Err(error) = self.session_store_worker.shutdown() {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    fn discard_lifecycle_transitions(&mut self) {
        let _ = self.lifecycle.take_transitions();
    }
}

impl Drop for RuntimeComponents {
    fn drop(&mut self) {
        let _ = self.shutdown(None);
    }
}
