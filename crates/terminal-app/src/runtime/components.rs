use std::sync::Arc;

use conversation_runtime::{ModelRefreshWorker, RuntimeEventNotifier};
use tool_runtime::ToolExecutorRegistry;

use super::{
    AppRuntimeOptions,
    agent::{AgentRuntime, NativeAgentRuntime},
    context_budget_worker::ContextBudgetWorker,
    effect_scope::{EffectScope, EffectScopeSnapshot},
    lifecycle::{CapabilityKey, ComponentDefinition, ComponentGraph, ReconciliationReport},
    llm_port::{LlmPort, ProviderRegistrations},
    permission_policy::{
        ApprovalProviderRegistration, InteractiveApprovalProviderFactory, PermissionPolicy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    },
    prompt_assembly::{PromptAssembly, PromptRegistration},
    session_port::{SessionBackendViews, SessionPortHost},
    session_tools_for_manager,
    session_worker::SessionStoreWorker,
    tool_catalog::{ToolCatalog, ToolRegistration},
    workspace_tools::conversation_workspace_tool_catalog,
};
use terminal_ui::RuntimeWake;

#[derive(Clone, Copy)]
struct CapabilityOwner {
    component_id: &'static str,
    capability: &'static str,
}

const APPROVAL_PROVIDER: CapabilityOwner = CapabilityOwner {
    component_id: "approval_provider",
    capability: "approval_provider",
};
const LLM_PORT: CapabilityOwner = CapabilityOwner {
    component_id: "llm_port",
    capability: "llm_port",
};
const MODEL_CATALOG: CapabilityOwner = CapabilityOwner {
    component_id: "llm_port",
    capability: "model_catalog",
};
const PERMISSION_POLICY: CapabilityOwner = CapabilityOwner {
    component_id: "permission_policy",
    capability: "permission_policy",
};
const PROMPT_ASSEMBLY: CapabilityOwner = CapabilityOwner {
    component_id: "prompt_assembly",
    capability: "prompt_assembly",
};
const RUNTIME_EVENT_STREAM: CapabilityOwner = CapabilityOwner {
    component_id: "runtime_event_stream",
    capability: "runtime_event_stream",
};
const RUNTIME_WAKE: CapabilityOwner = CapabilityOwner {
    component_id: "runtime_wake_binding",
    capability: "runtime_wake",
};
const SESSION_PERSISTENCE: CapabilityOwner = CapabilityOwner {
    component_id: "session_persistence",
    capability: "session_persistence",
};
const TOOL_CATALOG: CapabilityOwner = CapabilityOwner {
    component_id: "tool_catalog",
    capability: "tool_catalog",
};

/// `RuntimeComponents` 是 coordinator 的长期 runtime owner。
///
/// 它把 native Agent adapter、host workers、tool view、notifier 和 lifecycle graph 放在
/// 同一所有权边界，让 reset/shutdown 不再依赖 coordinator 手工枚举底层 conversation
/// resources。
pub(super) struct RuntimeComponents {
    pub(super) agent_runtime: NativeAgentRuntime,
    pub(super) model_refresh: ModelRefreshWorker,
    pub(super) llm_port: LlmPort,
    pub(super) permission_policy: PermissionPolicy,
    permission_provider_id: String,
    pub(super) tool_catalog: ToolCatalog,
    pub(super) prompt_assembly: PromptAssembly,
    pub(super) session_workspace_tools: ToolExecutorRegistry,
    pub(super) session_port: Option<SessionPortHost>,
    pub(super) session_backend_views: Option<SessionBackendViews>,
    pub(super) session_store_worker: SessionStoreWorker,
    pub(super) context_budget_worker: ContextBudgetWorker,
    pub(super) runtime_event_notifier: RuntimeEventNotifier,
    effect_scope: EffectScope,
    runtime_wake_scope: Option<EffectScope>,
    llm_port_scope: Option<EffectScope>,
    permission_policy_scope: Option<EffectScope>,
    tool_catalog_scope: Option<EffectScope>,
    prompt_assembly_scope: Option<EffectScope>,
    session_backend_scope: Option<EffectScope>,
    pub(super) lifecycle: ComponentGraph,
    is_shutdown: bool,
}

impl RuntimeComponents {
    pub(super) fn new(options: &mut AppRuntimeOptions) -> Result<Self, String> {
        let effect_scope = EffectScope::default();
        let runtime_event_notifier = RuntimeEventNotifier::default();
        let permission_policy = PermissionPolicy::new(runtime_event_notifier.clone());
        let approval_registration = permission_policy
            .register(
                "terminal-runtime",
                TERMINAL_APPROVAL_PROVIDER_ID,
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .map_err(|error| dispose_scope_tree_after_error(error.to_string(), &effect_scope))?;
        let permission_policy_scope =
            register_permission_policy_scope(&effect_scope, approval_registration)
                .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let llm_port = LlmPort::new();
        let provider_registrations = llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| dispose_scope_tree_after_error(error.to_string(), &effect_scope))?;
        let llm_port_scope = register_llm_port_scope(&effect_scope, provider_registrations)
            .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let (tool_catalog, tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
        )
        .map_err(|error| dispose_scope_tree_after_error(error.to_string(), &effect_scope))?;
        // initial composition 先安装 inverse，再把任何 catalog snapshot 交给 consumer；
        // 后续构造失败时显式关闭 root tree，完整回滚此前 registration。
        let tool_catalog_scope = register_tool_catalog_scope(&effect_scope, tool_registration)
            .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let (prompt_assembly, prompt_registration) = PromptAssembly::adopt_manager(
            "workspace-prompt",
            options.initial_prompt_assembly.clone(),
        )
        .map_err(|error| dispose_scope_tree_after_error(error.to_string(), &effect_scope))?;
        let prompt_assembly_scope =
            register_prompt_assembly_scope(&effect_scope, prompt_registration)
                .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let (session_port, session_backend_views, session_backend_scope) =
            mount_session_backend(&effect_scope, options.session_store.take())
                .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let prompt_assembly_snapshot = prompt_assembly.session_snapshot();
        let prompt_assembly_tool_definitions = tool_catalog.definitions();
        let session_workspace_tools =
            session_tools_for_manager(&tool_catalog, prompt_assembly_snapshot.manager.as_ref());
        let session_store_worker = SessionStoreWorker::new(runtime_event_notifier.clone());
        let context_budget_worker = ContextBudgetWorker::new(runtime_event_notifier.clone())
            .map_err(|error| dispose_scope_tree_after_error(error.to_string(), &effect_scope))?;
        let agent_runtime = NativeAgentRuntime::new(
            options,
            session_workspace_tools.clone(),
            prompt_assembly_tool_definitions,
            prompt_assembly_snapshot,
            session_backend_views
                .as_ref()
                .map(|views| Arc::clone(&views.port)),
            runtime_event_notifier.clone(),
            llm_port.clone(),
            permission_policy.clone(),
            TERMINAL_APPROVAL_PROVIDER_ID,
        )
        .map_err(|error| dispose_scope_tree_after_error(error, &effect_scope))?;
        let model_refresh = ModelRefreshWorker::new(runtime_event_notifier.clone());
        let has_session_backend = session_backend_views.is_some();
        let mut components = Self {
            agent_runtime,
            model_refresh,
            llm_port,
            permission_policy,
            permission_provider_id: TERMINAL_APPROVAL_PROVIDER_ID.to_string(),
            tool_catalog,
            prompt_assembly,
            session_workspace_tools,
            session_port,
            session_backend_views,
            session_store_worker,
            context_budget_worker,
            runtime_event_notifier,
            effect_scope,
            runtime_wake_scope: None,
            llm_port_scope: Some(llm_port_scope),
            permission_policy_scope: Some(permission_policy_scope),
            tool_catalog_scope: Some(tool_catalog_scope),
            prompt_assembly_scope: Some(prompt_assembly_scope),
            session_backend_scope,
            lifecycle: ComponentGraph::default(),
            is_shutdown: false,
        };
        if let Err(error) = components.initialize_lifecycle(has_session_backend) {
            let error = match components.shutdown() {
                Ok(()) => error,
                Err(rollback_error) => format!("{error}; {rollback_error}"),
            };
            return Err(error);
        }
        Ok(components)
    }

    fn initialize_lifecycle(&mut self, has_session_backend: bool) -> Result<(), String> {
        for definition in runtime_component_definitions() {
            declare_component(&mut self.lifecycle, definition)?;
        }
        for capability in [
            APPROVAL_PROVIDER,
            LLM_PORT,
            MODEL_CATALOG,
            PERMISSION_POLICY,
            PROMPT_ASSEMBLY,
            RUNTIME_EVENT_STREAM,
            TOOL_CATALOG,
        ] {
            publish_capability(&mut self.lifecycle, capability)?;
        }
        if has_session_backend {
            publish_capability(&mut self.lifecycle, SESSION_PERSISTENCE)?;
        }
        Ok(())
    }

    pub(super) fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        self.remove_runtime_wake()?;
        let mut binding = self
            .runtime_event_notifier
            .bind_callback(move || wake.wake());
        let scope = self
            .effect_scope
            .child("ui_runtime_bridge")
            .map_err(|error| error.to_string())?;
        scope
            .register("runtime_wake_binding", move || {
                binding.dispose();
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        self.runtime_wake_scope = Some(scope);
        if let Err(error) = publish_capability(&mut self.lifecycle, RUNTIME_WAKE) {
            return Err(dispose_scopes_after_error(
                error,
                self.runtime_wake_scope.take(),
            ));
        }
        Ok(())
    }

    pub(super) fn remove_runtime_wake(&mut self) -> Result<(), String> {
        remove_capability(&mut self.lifecycle, RUNTIME_WAKE)?;
        dispose_scope(&mut self.runtime_wake_scope)?;
        Ok(())
    }

    /// 返回 composition root 下的 active component scope；root identity 不进入诊断投影。
    pub(super) fn effect_scope_snapshots(&self) -> Vec<EffectScopeSnapshot> {
        self.effect_scope
            .snapshot()
            .map(|snapshot| snapshot.children)
            .unwrap_or_default()
    }

    pub(super) fn reset_after_clear(&mut self, options: &AppRuntimeOptions) -> Result<(), String> {
        self.reset_after_clear_with_native_mount_check(options, || Ok(()))
    }

    /// 在全部 consumer 与旧 worker quiesce 后替换当前 session backend。
    #[allow(dead_code)]
    pub(super) fn replace_session_backend(
        &mut self,
        options: &AppRuntimeOptions,
        store: Arc<dyn session_store::SessionStore>,
    ) -> Result<(), String> {
        self.replace_session_backend_with_native_mount_check(options, store, || Ok(()))
    }

    #[allow(dead_code)]
    fn replace_session_backend_with_native_mount_check(
        &mut self,
        options: &AppRuntimeOptions,
        store: Arc<dyn session_store::SessionStore>,
        native_mount_check: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        validate_capability_replacement(&self.lifecycle, SESSION_PERSISTENCE)?;
        remove_capability(&mut self.lifecycle, SESSION_PERSISTENCE)?;

        let mut failures = Vec::new();
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        if self.session_store_worker.is_running()
            && let Some(views) = self.session_backend_views.clone()
            && let Err(error) = self.session_store_worker.flush_all(views)
        {
            failures.push(error);
        }
        if let Err(error) = self.session_store_worker.shutdown() {
            failures.push(error);
        }
        if let Some(session_port) = &self.session_port {
            session_port.deactivate();
        }
        if let Err(error) = dispose_scope(&mut self.session_backend_scope) {
            failures.push(error);
        }
        self.session_port = None;
        self.session_backend_views = None;
        if !failures.is_empty() {
            let cleanup_error = failures.join("; ");
            self.restore_ephemeral_session_consumers(options)
                .map_err(|fallback_error| format!("{cleanup_error}; {fallback_error}"))?;
            return Err(cleanup_error);
        }

        let (fresh_session_port, fresh_views, fresh_scope) =
            match mount_session_backend(&self.effect_scope, Some(store)) {
                Ok(mounted) => mounted,
                Err(error) => {
                    self.restore_ephemeral_session_consumers(options)
                        .map_err(|fallback_error| format!("{error}; {fallback_error}"))?;
                    return Err(error);
                }
            };
        let Some(fresh_views) = fresh_views else {
            return Err("session backend mount did not produce views".to_string());
        };
        let fresh_agent_runtime = match native_mount_check().and_then(|()| {
            self.fresh_native_agent_runtime(options, Some(Arc::clone(&fresh_views.port)))
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                if let Some(session_port) = &fresh_session_port {
                    session_port.deactivate();
                }
                let error = dispose_scopes_after_error(error, fresh_scope);
                self.restore_ephemeral_session_consumers(options)
                    .map_err(|fallback_error| format!("{error}; {fallback_error}"))?;
                return Err(error);
            }
        };

        self.session_store_worker = SessionStoreWorker::new(self.runtime_event_notifier.clone());
        self.agent_runtime = fresh_agent_runtime;
        self.session_port = fresh_session_port;
        self.session_backend_views = Some(fresh_views);
        self.session_backend_scope = fresh_scope;
        replace_capability(&mut self.lifecycle, SESSION_PERSISTENCE);
        Ok(())
    }

    fn fresh_native_agent_runtime(
        &self,
        options: &AppRuntimeOptions,
        session_port: Option<Arc<dyn session_store::SessionPort>>,
    ) -> Result<NativeAgentRuntime, String> {
        NativeAgentRuntime::new(
            options,
            self.session_workspace_tools.clone(),
            self.tool_catalog.definitions(),
            self.prompt_assembly.session_snapshot(),
            session_port,
            self.runtime_event_notifier.clone(),
            self.llm_port.clone(),
            self.permission_policy.clone(),
            self.permission_provider_id.as_str(),
        )
    }

    fn restore_ephemeral_session_consumers(
        &mut self,
        options: &AppRuntimeOptions,
    ) -> Result<(), String> {
        let fresh_agent_runtime = self
            .fresh_native_agent_runtime(options, None)
            .map_err(|error| format!("restore ephemeral Agent after backend failure: {error}"))?;
        self.agent_runtime = fresh_agent_runtime;
        self.session_store_worker = SessionStoreWorker::new(self.runtime_event_notifier.clone());
        Ok(())
    }

    /// Replaces the approval provider generation while keeping the other runtime capabilities
    /// intact. The old Native adapter is quiesced before its policy is deactivated so no turn can
    /// retain a handler into the removed provider generation.
    #[allow(dead_code)]
    pub(super) fn replace_permission_provider(
        &mut self,
        options: &AppRuntimeOptions,
        owner: impl Into<String>,
        provider_id: impl Into<String>,
        factory: Arc<dyn super::permission_policy::ApprovalProviderFactory>,
    ) -> Result<(), String> {
        self.replace_permission_provider_with_native_mount_check(
            options,
            owner,
            provider_id,
            factory,
            || Ok(()),
        )
    }

    #[allow(dead_code)]
    fn replace_permission_provider_with_native_mount_check(
        &mut self,
        options: &AppRuntimeOptions,
        owner: impl Into<String>,
        provider_id: impl Into<String>,
        factory: Arc<dyn super::permission_policy::ApprovalProviderFactory>,
        native_mount_check: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let replaced = [APPROVAL_PROVIDER, PERMISSION_POLICY];
        for capability in replaced {
            validate_capability_replacement(&self.lifecycle, capability)?;
        }
        for capability in replaced {
            remove_capability(&mut self.lifecycle, capability)?;
        }

        let mut failures = Vec::new();
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        self.permission_policy.deactivate();
        if let Err(error) = dispose_scope(&mut self.permission_policy_scope) {
            failures.push(error);
        }
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }

        let provider_id = provider_id.into();
        let fresh_policy = PermissionPolicy::new(self.runtime_event_notifier.clone());
        let fresh_registration = fresh_policy
            .register(owner, provider_id.clone(), factory)
            .map_err(|error| error.to_string())?;
        let fresh_policy_scope =
            match register_permission_policy_scope(&self.effect_scope, fresh_registration) {
                Ok(scope) => scope,
                Err(error) => {
                    fresh_policy.deactivate();
                    return Err(error);
                }
            };

        let fresh_agent_runtime = match native_mount_check().and_then(|()| {
            NativeAgentRuntime::new(
                options,
                self.session_workspace_tools.clone(),
                self.tool_catalog.definitions(),
                self.prompt_assembly.session_snapshot(),
                self.session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                self.runtime_event_notifier.clone(),
                self.llm_port.clone(),
                fresh_policy.clone(),
                provider_id.clone(),
            )
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                let error = dispose_scopes_after_error(error, [fresh_policy_scope]);
                fresh_policy.deactivate();
                return Err(error);
            }
        };

        self.permission_policy = fresh_policy;
        self.permission_provider_id = provider_id;
        self.permission_policy_scope = Some(fresh_policy_scope);
        self.agent_runtime = fresh_agent_runtime;
        for capability in replaced {
            replace_capability(&mut self.lifecycle, capability);
        }
        Ok(())
    }

    fn reset_after_clear_with_native_mount_check(
        &mut self,
        options: &AppRuntimeOptions,
        native_mount_check: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let replaced = [LLM_PORT, MODEL_CATALOG, PROMPT_ASSEMBLY, TOOL_CATALOG];
        for capability in replaced {
            validate_capability_replacement(&self.lifecycle, capability)?;
        }
        for capability in replaced {
            remove_capability(&mut self.lifecycle, capability)?;
        }

        let mut failures = Vec::new();
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        if let Err(error) = self.model_refresh.reset_after_clear() {
            failures.push(error);
        }
        self.context_budget_worker.cancel_pending();
        self.session_workspace_tools = ToolExecutorRegistry::new();
        let current_prompt_assembly = self.prompt_assembly.manager_snapshot();
        self.prompt_assembly.deactivate();
        self.llm_port.deactivate();
        if let Err(error) = dispose_scope(&mut self.prompt_assembly_scope) {
            failures.push(error);
        }
        if let Err(error) = dispose_scope(&mut self.tool_catalog_scope) {
            failures.push(error);
        }
        if let Err(error) = dispose_scope(&mut self.llm_port_scope) {
            failures.push(error);
        }
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }

        let fresh_llm_port = LlmPort::new();
        let fresh_provider_registrations = fresh_llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let fresh_llm_port_scope =
            register_llm_port_scope(&self.effect_scope, fresh_provider_registrations)?;
        let (fresh_tool_catalog, fresh_tool_registration) =
            match conversation_workspace_tool_catalog(
                &options.managed_ripgrep,
                &options.hunea_config_dir,
            ) {
                Ok(catalog) => catalog,
                Err(error) => {
                    return Err(dispose_scopes_after_error(
                        error.to_string(),
                        [fresh_llm_port_scope],
                    ));
                }
            };
        let fresh_tool_catalog_scope =
            match register_tool_catalog_scope(&self.effect_scope, fresh_tool_registration) {
                Ok(scope) => scope,
                Err(error) => {
                    return Err(dispose_scopes_after_error(error, [fresh_llm_port_scope]));
                }
            };
        let prompt_assembly_tool_definitions = fresh_tool_catalog.definitions();
        let (fresh_prompt_assembly, fresh_prompt_registration) =
            match PromptAssembly::adopt_manager("workspace-prompt", current_prompt_assembly) {
                Ok(prompt_assembly) => prompt_assembly,
                Err(error) => {
                    return Err(dispose_scopes_after_error(
                        error.to_string(),
                        [fresh_tool_catalog_scope, fresh_llm_port_scope],
                    ));
                }
            };
        let fresh_prompt_assembly_scope =
            match register_prompt_assembly_scope(&self.effect_scope, fresh_prompt_registration) {
                Ok(scope) => scope,
                Err(error) => {
                    return Err(dispose_scopes_after_error(
                        error,
                        [fresh_tool_catalog_scope, fresh_llm_port_scope],
                    ));
                }
            };
        let fresh_prompt_assembly_snapshot = fresh_prompt_assembly.session_snapshot();
        let session_workspace_tools = session_tools_for_manager(
            &fresh_tool_catalog,
            fresh_prompt_assembly_snapshot.manager.as_ref(),
        );
        // 旧 adapter 完全 quiescent 后才创建新 generation，避免 reset 期间存在两个
        // native worker path；构造失败时 capability 仍保持 removed，不发布半成品。
        let fresh_agent_runtime = match native_mount_check().and_then(|()| {
            NativeAgentRuntime::new(
                options,
                session_workspace_tools.clone(),
                prompt_assembly_tool_definitions,
                fresh_prompt_assembly_snapshot,
                self.session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                self.runtime_event_notifier.clone(),
                fresh_llm_port.clone(),
                self.permission_policy.clone(),
                self.permission_provider_id.as_str(),
            )
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                return Err(dispose_scopes_after_error(
                    error,
                    [
                        fresh_prompt_assembly_scope,
                        fresh_tool_catalog_scope,
                        fresh_llm_port_scope,
                    ],
                ));
            }
        };
        self.agent_runtime = fresh_agent_runtime;
        self.llm_port = fresh_llm_port;
        self.tool_catalog = fresh_tool_catalog;
        self.prompt_assembly = fresh_prompt_assembly;
        self.tool_catalog_scope = Some(fresh_tool_catalog_scope);
        self.llm_port_scope = Some(fresh_llm_port_scope);
        self.prompt_assembly_scope = Some(fresh_prompt_assembly_scope);
        self.session_workspace_tools = session_workspace_tools;
        for capability in replaced {
            replace_capability(&mut self.lifecycle, capability);
        }
        Ok(())
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        if self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        let mut failures = Vec::new();
        if let Err(error) = self.remove_runtime_wake() {
            failures.push(error);
        }
        for capability in [
            APPROVAL_PROVIDER,
            LLM_PORT,
            MODEL_CATALOG,
            PERMISSION_POLICY,
            PROMPT_ASSEMBLY,
            RUNTIME_EVENT_STREAM,
            TOOL_CATALOG,
            SESSION_PERSISTENCE,
        ] {
            if let Err(error) = remove_capability(&mut self.lifecycle, capability) {
                failures.push(error);
            }
        }
        if let Err(error) = self.agent_runtime.shutdown() {
            failures.push(error.to_string());
        }
        if let Err(error) = self.model_refresh.shutdown() {
            failures.push(error);
        }
        self.permission_policy.deactivate();
        if let Err(error) = dispose_scope(&mut self.permission_policy_scope) {
            failures.push(error);
        }
        self.session_workspace_tools = ToolExecutorRegistry::new();
        self.prompt_assembly.deactivate();
        self.llm_port.deactivate();
        if let Err(error) = dispose_scope(&mut self.prompt_assembly_scope) {
            failures.push(error);
        }
        if let Err(error) = dispose_scope(&mut self.tool_catalog_scope) {
            failures.push(error);
        }
        if let Err(error) = dispose_scope(&mut self.llm_port_scope) {
            failures.push(error);
        }
        if let Err(error) = self.context_budget_worker.shutdown() {
            failures.push(error);
        }
        if self.session_store_worker.is_running()
            && let Some(views) = self.session_backend_views.clone()
            && let Err(error) = self.session_store_worker.flush_all(views)
        {
            failures.push(error);
        }
        if let Err(error) = self.session_store_worker.shutdown() {
            failures.push(error);
        }
        if let Some(session_port) = &self.session_port {
            session_port.deactivate();
        }
        if let Err(error) = dispose_scope(&mut self.session_backend_scope) {
            failures.push(error);
        }
        self.session_backend_views = None;
        self.session_port = None;
        if let Some(error) = self.effect_scope.dispose().error_message() {
            failures.push(error);
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

fn runtime_component_definitions() -> Vec<ComponentDefinition> {
    vec![
        ComponentDefinition::new(APPROVAL_PROVIDER.component_id)
            .provides(APPROVAL_PROVIDER.capability),
        ComponentDefinition::new(LLM_PORT.component_id)
            .provides(LLM_PORT.capability)
            .provides(MODEL_CATALOG.capability),
        ComponentDefinition::new(RUNTIME_EVENT_STREAM.component_id)
            .provides(RUNTIME_EVENT_STREAM.capability),
        ComponentDefinition::new(RUNTIME_WAKE.component_id).provides(RUNTIME_WAKE.capability),
        ComponentDefinition::new(SESSION_PERSISTENCE.component_id)
            .provides(SESSION_PERSISTENCE.capability),
        ComponentDefinition::new(TOOL_CATALOG.component_id).provides(TOOL_CATALOG.capability),
        ComponentDefinition::new(PERMISSION_POLICY.component_id)
            .requires(APPROVAL_PROVIDER.capability)
            .provides(PERMISSION_POLICY.capability),
        ComponentDefinition::new(PROMPT_ASSEMBLY.component_id)
            .requires(TOOL_CATALOG.capability)
            .observes(SESSION_PERSISTENCE.capability)
            .provides(PROMPT_ASSEMBLY.capability),
        ComponentDefinition::new("native_agent_runtime")
            .requires(LLM_PORT.capability)
            .requires(MODEL_CATALOG.capability)
            .requires(PERMISSION_POLICY.capability)
            .requires(PROMPT_ASSEMBLY.capability)
            .requires(TOOL_CATALOG.capability)
            .observes(SESSION_PERSISTENCE.capability),
        ComponentDefinition::new("model_refresh")
            .requires(LLM_PORT.capability)
            .requires(MODEL_CATALOG.capability),
        ComponentDefinition::new("ui_runtime_bridge")
            .requires(RUNTIME_EVENT_STREAM.capability)
            .requires(RUNTIME_WAKE.capability),
    ]
}

fn declare_component(
    graph: &mut ComponentGraph,
    definition: ComponentDefinition,
) -> Result<(), String> {
    let report = graph
        .declare(definition)
        .map_err(|error| error.to_string())?;
    acknowledge_activations(graph, report);
    Ok(())
}

fn publish_capability(
    graph: &mut ComponentGraph,
    capability: CapabilityOwner,
) -> Result<(), String> {
    let report = graph
        .add_capability(capability.component_id, capability.capability)
        .map_err(|error| error.to_string())?;
    acknowledge_activations(graph, report);
    Ok(())
}

fn remove_capability(
    graph: &mut ComponentGraph,
    capability: CapabilityOwner,
) -> Result<(), String> {
    graph
        .remove_capability(
            capability.component_id,
            &CapabilityKey::from(capability.capability),
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validate_capability_replacement(
    graph: &ComponentGraph,
    capability: CapabilityOwner,
) -> Result<(), String> {
    graph
        .validate_replacement(
            capability.component_id,
            &CapabilityKey::from(capability.capability),
        )
        .map_err(|error| error.to_string())
}

fn replace_capability(graph: &mut ComponentGraph, capability: CapabilityOwner) {
    let report = graph
        .replace_capability(
            capability.component_id,
            &CapabilityKey::from(capability.capability),
        )
        .expect("preflighted graph replacement must remain valid");
    acknowledge_activations(graph, report);
}

fn acknowledge_activations(graph: &mut ComponentGraph, report: ReconciliationReport) {
    for token in report.activation_requests {
        graph
            .complete_activation(token)
            .expect("fresh graph activation token must remain current");
    }
}

fn register_tool_catalog_scope(
    root_scope: &EffectScope,
    mut registration: ToolRegistration,
) -> Result<EffectScope, String> {
    let scope = root_scope
        .child("tool_catalog")
        .map_err(|error| error.to_string())?;
    scope
        .register("tool_registrations", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(scope)
}

fn register_llm_port_scope(
    root_scope: &EffectScope,
    mut registrations: ProviderRegistrations,
) -> Result<EffectScope, String> {
    let scope = root_scope
        .child("llm_port")
        .map_err(|error| error.to_string())?;
    scope
        .register("provider_registrations", move || {
            registrations.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(scope)
}

fn register_permission_policy_scope(
    root_scope: &EffectScope,
    mut registration: ApprovalProviderRegistration,
) -> Result<EffectScope, String> {
    let scope = root_scope
        .child("permission_policy")
        .map_err(|error| error.to_string())?;
    scope
        .register("approval_provider_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(scope)
}

fn register_prompt_assembly_scope(
    root_scope: &EffectScope,
    mut registration: PromptRegistration,
) -> Result<EffectScope, String> {
    let scope = root_scope
        .child("prompt_assembly")
        .map_err(|error| error.to_string())?;
    scope
        .register("prompt_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(scope)
}

type MountedSessionBackend = (
    Option<SessionPortHost>,
    Option<SessionBackendViews>,
    Option<EffectScope>,
);

fn mount_session_backend(
    root_scope: &EffectScope,
    store: Option<Arc<dyn session_store::SessionStore>>,
) -> Result<MountedSessionBackend, String> {
    let Some(store) = store else {
        return Ok((None, None, None));
    };
    let session_port = SessionPortHost::new();
    let registration = session_port
        .register("terminal-runtime", "configured-session-store", store)
        .map_err(|error| error.to_string())?;
    let scope = register_session_backend_scope(root_scope, registration)?;
    let views = match session_port.views() {
        Ok(views) => views,
        Err(error) => {
            session_port.deactivate();
            return Err(dispose_scopes_after_error(error.to_string(), [scope]));
        }
    };
    Ok((Some(session_port), Some(views), Some(scope)))
}

fn register_session_backend_scope(
    root_scope: &EffectScope,
    mut registration: super::session_port::SessionBackendRegistration,
) -> Result<EffectScope, String> {
    let scope = root_scope
        .child("session_persistence")
        .map_err(|error| error.to_string())?;
    scope
        .register("backend_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(scope)
}

fn dispose_scope(scope: &mut Option<EffectScope>) -> Result<(), String> {
    scope
        .take()
        .and_then(|scope| scope.dispose().error_message())
        .map_or(Ok(()), Err)
}

fn dispose_scopes_after_error(
    error: String,
    scopes: impl IntoIterator<Item = EffectScope>,
) -> String {
    let mut failures = vec![error];
    for scope in scopes {
        if let Some(error) = scope.dispose().error_message() {
            failures.push(error);
        }
    }
    failures.join("; ")
}

fn dispose_scope_tree_after_error(error: String, root_scope: &EffectScope) -> String {
    let mut failures = vec![error];
    if let Some(error) = root_scope.dispose().error_message() {
        failures.push(error);
    }
    failures.join("; ")
}

impl Drop for RuntimeComponents {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
    };

    use super::*;
    use crate::runtime::lifecycle::ComponentState;
    use runtime_domain::prompt_assembly::{
        PromptPreludeSection, PromptSourceKind, PromptSourceOrigin,
    };

    fn manager_with_section(
        reference_id: &str,
        body: &str,
    ) -> runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot {
        let mut manager = runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot::default();
        manager
            .resolution
            .prelude
            .sections
            .push(PromptPreludeSection {
                reference_id: reference_id.to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: reference_id.to_string(),
                origin: Some(PromptSourceOrigin::Project),
                body: body.to_string(),
            });
        manager
    }

    fn options_with_provider() -> AppRuntimeOptions {
        AppRuntimeOptions {
            loaded_models: conversation_runtime::models::LoadedModelCatalog {
                provider_configs: vec![conversation_runtime::models::LoadedProviderConfig::new(
                    "local",
                    runtime_domain::provider::ProviderKind::OpenAiCompatible,
                    Some("http://localhost:11434/v1".to_string()),
                    None,
                    None,
                    true,
                )],
                ..conversation_runtime::models::LoadedModelCatalog::default()
            },
            ..AppRuntimeOptions::default()
        }
    }

    struct BackendStateCheckingFlush {
        host: SessionPortHost,
        did_flush_while_mounted: Arc<AtomicBool>,
    }

    struct FailingFlush;

    impl session_store::SessionFlushStore for FailingFlush {
        fn flush<'a>(
            &'a self,
            _session_id: &'a session_store::SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(session_store::SessionStoreError::ConfigurationError {
                    message: "injected flush failure".to_string(),
                })
            })
        }

        fn flush_all<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(session_store::SessionStoreError::ConfigurationError {
                    message: "injected flush failure".to_string(),
                })
            })
        }
    }

    impl session_store::SessionFlushStore for BackendStateCheckingFlush {
        fn flush<'a>(
            &'a self,
            _session_id: &'a session_store::SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(()) })
        }

        fn flush_all<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            let is_mounted = self.host.inspection_snapshot().is_some();
            self.did_flush_while_mounted
                .store(is_mounted, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn shutdown_clears_prompt_and_tool_generations() {
        let mut options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .shutdown()
            .expect("shutdown should dispose all runtime effects");

        assert!(components.prompt_assembly.manager_snapshot().is_none());
        assert!(components.prompt_assembly.inspection_snapshot().is_empty());
        assert!(components.tool_catalog.definitions().is_empty());
        assert!(components.llm_port.inspection_snapshot().is_empty());
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Pending)
        );
    }

    #[test]
    fn shutdown_flushes_session_worker_before_backend_inverse() {
        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut options = AppRuntimeOptions {
            session_store: Some(store.clone()),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        assert!(
            options.session_store.is_none(),
            "bootstrap options must transfer the raw store into RuntimeComponents"
        );
        let host = components
            .session_port
            .clone()
            .expect("configured backend should have a host");
        let did_flush_while_mounted = Arc::new(AtomicBool::new(false));
        components
            .session_backend_views
            .as_mut()
            .expect("configured backend should expose views")
            .flush = Arc::new(BackendStateCheckingFlush {
            host,
            did_flush_while_mounted: Arc::clone(&did_flush_while_mounted),
        });

        components
            .shutdown()
            .expect("shutdown should flush and dispose the backend");

        assert!(did_flush_while_mounted.load(Ordering::SeqCst));
        assert!(components.session_port.is_none());
        assert!(components.session_backend_views.is_none());
        assert_eq!(
            Arc::strong_count(&store),
            1,
            "shutdown must release every backend lease before returning"
        );
    }

    #[test]
    fn missing_session_backend_keeps_optional_consumers_ephemeral_and_active() {
        let mut options = options_with_provider();
        let components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        assert!(components.session_port.is_none());
        assert!(components.session_backend_views.is_none());
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );
        assert_eq!(
            components.lifecycle.optional_available(
                "native_agent_runtime",
                &CapabilityKey::from("session_persistence"),
            ),
            Some(false)
        );
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Active)
        );
        assert!(components.agent_runtime.is_idle_empty_session());
        assert!(!components.agent_runtime.is_shutdown_for_test());
    }

    #[test]
    fn reset_rebinds_the_current_live_prompt_manager() {
        let mut options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .prompt_assembly
            .replace_manager(Some(manager_with_section("live", "live body")))
            .expect("live manager should be replaceable");

        components
            .reset_after_clear(&options)
            .expect("reset should install a new generation");

        let snapshot = components
            .prompt_assembly
            .session_snapshot()
            .prompt_prelude
            .expect("fresh generation should keep the live manager");
        assert_eq!(snapshot.sections[0].reference_id, "live");
        assert_eq!(snapshot.sections[0].body, "live body");
    }

    #[test]
    fn failed_native_mount_reverts_fresh_provider_prompt_and_tool_effects() {
        let mut options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        let error = components
            .reset_after_clear_with_native_mount_check(&options, || {
                Err("injected native mount failure".to_string())
            })
            .expect_err("injected native mount failure should abort publication");

        assert_eq!(error, "injected native mount failure");
        assert!(components.tool_catalog.definitions().is_empty());
        assert!(components.prompt_assembly.manager_snapshot().is_none());
        assert!(components.prompt_assembly.inspection_snapshot().is_empty());
        assert!(components.llm_port.inspection_snapshot().is_empty());
        assert_eq!(
            components
                .session_workspace_tools
                .definitions()
                .definitions()
                .count(),
            0
        );
        for capability in [
            "llm_port",
            "model_catalog",
            "prompt_assembly",
            "tool_catalog",
        ] {
            assert!(
                !components
                    .lifecycle
                    .has_capability(&CapabilityKey::from(capability))
            );
        }
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Pending)
        );
    }

    #[test]
    fn failed_provider_remount_keeps_old_generation_removed_and_dependents_pending() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let duplicate_options = AppRuntimeOptions {
            loaded_models: conversation_runtime::models::LoadedModelCatalog {
                provider_configs: vec![
                    conversation_runtime::models::LoadedProviderConfig::new(
                        "duplicate",
                        runtime_domain::provider::ProviderKind::OpenAiCompatible,
                        Some("http://localhost:11434/v1".to_string()),
                        None,
                        None,
                        true,
                    ),
                    conversation_runtime::models::LoadedProviderConfig::new(
                        "duplicate",
                        runtime_domain::provider::ProviderKind::OpenAiCompatible,
                        Some("http://localhost:11435/v1".to_string()),
                        None,
                        None,
                        true,
                    ),
                ],
                ..conversation_runtime::models::LoadedModelCatalog::default()
            },
            ..AppRuntimeOptions::default()
        };

        let error = components
            .reset_after_clear(&duplicate_options)
            .expect_err("duplicate provider remount should fail transactionally");

        assert!(error.contains("provider duplicate is already registered"));
        assert!(components.llm_port.inspection_snapshot().is_empty());
        for capability in [
            "llm_port",
            "model_catalog",
            "prompt_assembly",
            "tool_catalog",
        ] {
            assert!(
                !components
                    .lifecycle
                    .has_capability(&CapabilityKey::from(capability))
            );
        }
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state("model_refresh"),
            Some(ComponentState::Pending)
        );
    }

    #[test]
    fn permission_provider_replacement_quiesces_old_generation_before_publish() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let before = components.permission_policy.inspection_snapshot();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].provider_id, TERMINAL_APPROVAL_PROVIDER_ID);

        components
            .replace_permission_provider(
                &options,
                "replacement-owner",
                "replacement-provider",
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .expect("replacement provider should mount");

        let after = components.permission_policy.inspection_snapshot();
        assert_eq!(
            after
                .iter()
                .map(|provider| provider.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["replacement-provider"]
        );
        assert_eq!(
            components.lifecycle.state("permission_policy"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Active)
        );
        assert!(
            components
                .lifecycle
                .capabilities()
                .iter()
                .any(|capability| capability.key == "approval_provider")
        );
    }

    #[test]
    fn failed_session_backend_replacement_does_not_revive_old_generation() {
        let mut options = AppRuntimeOptions {
            session_store: Some(Arc::new(session_store::InMemorySessionStore::new())),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        assert!(options.session_store.is_none());
        assert!(components.session_backend_views.is_some());
        assert!(
            components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );

        let error = components
            .replace_session_backend_with_native_mount_check(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
                || Err("injected session Agent mount failure".to_string()),
            )
            .expect_err("injected session backend mount failure should abort publication");

        assert_eq!(error, "injected session Agent mount failure");
        assert!(components.session_backend_views.is_none());
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );
        assert!(components.agent_runtime.is_idle_empty_session());
        assert!(!components.agent_runtime.is_shutdown_for_test());
        assert!(components.session_store_worker.is_running());
    }

    #[test]
    fn failed_old_backend_flush_releases_it_and_restores_ephemeral_consumers() {
        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut options = AppRuntimeOptions {
            session_store: Some(store.clone()),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .session_backend_views
            .as_mut()
            .expect("configured backend should expose views")
            .flush = Arc::new(FailingFlush);

        let error = components
            .replace_session_backend(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
            )
            .expect_err("old backend flush failure must abort replacement");

        assert!(error.contains("injected flush failure"));
        assert!(components.session_port.is_none());
        assert!(components.session_backend_views.is_none());
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Active)
        );
        assert!(!components.agent_runtime.is_shutdown_for_test());
        assert!(components.session_store_worker.is_running());
        assert_eq!(
            Arc::strong_count(&store),
            1,
            "failed teardown must not retain the old backend generation"
        );
    }

    #[test]
    fn session_backend_replacement_publishes_a_fresh_generation() {
        let mut options = AppRuntimeOptions {
            session_store: Some(Arc::new(session_store::InMemorySessionStore::new())),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let before_generation = components
            .lifecycle
            .capabilities()
            .into_iter()
            .find(|capability| capability.key == "session_persistence")
            .expect("initial backend capability should be published")
            .generation;

        components
            .replace_session_backend(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
            )
            .expect("backend replacement should succeed");

        let after_generation = components
            .lifecycle
            .capabilities()
            .into_iter()
            .find(|capability| capability.key == "session_persistence")
            .expect("replacement backend capability should be published")
            .generation;
        assert_eq!(after_generation, before_generation + 1);
        assert!(components.session_backend_views.is_some());
        assert!(!components.agent_runtime.is_shutdown_for_test());
    }

    #[test]
    fn permission_provider_replacement_keeps_provider_id_across_reset() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .replace_permission_provider(
                &options,
                "replacement-owner",
                "replacement-provider",
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .expect("replacement provider should mount");
        components
            .reset_after_clear(&options)
            .expect("reset should preserve the active provider generation");

        assert_eq!(
            components.agent_runtime.permission_provider_id_for_test(),
            "replacement-provider"
        );
    }

    #[test]
    fn failed_permission_provider_replacement_does_not_revive_old_generation() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        let error = components
            .replace_permission_provider_with_native_mount_check(
                &options,
                "replacement-owner",
                "replacement-provider",
                Arc::new(InteractiveApprovalProviderFactory),
                || Err("injected permission native mount failure".to_string()),
            )
            .expect_err("injected native mount failure should abort replacement");

        assert_eq!(error, "injected permission native mount failure");
        assert!(
            components
                .permission_policy
                .inspection_snapshot()
                .is_empty()
        );
        for capability in ["approval_provider", "permission_policy"] {
            assert!(
                !components
                    .lifecycle
                    .has_capability(&CapabilityKey::from(capability))
            );
        }
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state("permission_policy"),
            Some(ComponentState::Pending)
        );
    }
}
