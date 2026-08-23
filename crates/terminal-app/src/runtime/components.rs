use std::sync::Arc;

use conversation_runtime::{ModelRefreshWorker, RuntimeEventNotifier};
use tool_runtime::ToolExecutorRegistry;

use super::{
    AppRuntimeOptions,
    agent::{AgentRuntime, NativeAgentRuntime},
    context_budget_worker::ContextBudgetWorker,
    lifecycle::{CapabilityKey, ComponentDefinition, ComponentGraph, EffectId, EffectScope},
    llm_port::{LlmPort, ProviderRegistrations},
    prompt_assembly::{PromptAssembly, PromptRegistration},
    session_tools_for_manager,
    session_worker::SessionStoreWorker,
    tool_catalog::{ToolCatalog, ToolRegistration},
    workspace_tools::conversation_workspace_tool_catalog,
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
    pub(super) llm_port: LlmPort,
    pub(super) tool_catalog: ToolCatalog,
    pub(super) prompt_assembly: PromptAssembly,
    pub(super) session_workspace_tools: ToolExecutorRegistry,
    pub(super) session_store_worker: SessionStoreWorker,
    pub(super) context_budget_worker: ContextBudgetWorker,
    pub(super) runtime_event_notifier: RuntimeEventNotifier,
    effect_scope: EffectScope,
    runtime_wake_effect: Option<EffectId>,
    llm_port_effect: Option<EffectId>,
    tool_catalog_effect: Option<EffectId>,
    prompt_assembly_effect: Option<EffectId>,
    pub(super) lifecycle: ComponentGraph,
    is_shutdown: bool,
}

impl RuntimeComponents {
    pub(super) fn new(options: &AppRuntimeOptions) -> Result<Self, String> {
        let effect_scope = EffectScope::default();
        let llm_port = LlmPort::new();
        let provider_registrations = llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let llm_port_effect = register_llm_port_effect(&effect_scope, provider_registrations)?;
        let (tool_catalog, tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
        )
        .map_err(|error| error.to_string())?;
        // initial composition 先安装 inverse，再把任何 catalog snapshot 交给 consumer；
        // 后续构造失败时 local scope Drop 会完整回滚 registration。
        let tool_catalog_effect = register_tool_catalog_effect(&effect_scope, tool_registration)?;
        let (prompt_assembly, prompt_registration) = PromptAssembly::adopt_manager(
            "workspace-prompt",
            options.initial_prompt_assembly.clone(),
        )
        .map_err(|error| error.to_string())?;
        let prompt_assembly_effect =
            register_prompt_assembly_effect(&effect_scope, prompt_registration)?;
        let prompt_assembly_snapshot = prompt_assembly.session_snapshot();
        let prompt_assembly_tool_definitions = tool_catalog.definitions();
        let session_workspace_tools =
            session_tools_for_manager(&tool_catalog, prompt_assembly_snapshot.manager.as_ref());
        let runtime_event_notifier = RuntimeEventNotifier::default();
        let agent_runtime = NativeAgentRuntime::new(
            options,
            session_workspace_tools.clone(),
            prompt_assembly_tool_definitions,
            prompt_assembly_snapshot,
            runtime_event_notifier.clone(),
            llm_port.clone(),
        )?;
        let mut lifecycle = ComponentGraph::default();
        lifecycle.declare(
            ComponentDefinition::new("native_agent_runtime")
                .requires("llm_port")
                .requires("model_catalog")
                .requires("prompt_assembly")
                .requires("tool_catalog")
                .observes("session_persistence"),
        );
        lifecycle.declare(
            ComponentDefinition::new("model_refresh")
                .requires("llm_port")
                .requires("model_catalog"),
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
        for capability in [
            "llm_port",
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
        lifecycle.take_transitions();
        Ok(Self {
            agent_runtime,
            model_refresh: ModelRefreshWorker::new(runtime_event_notifier.clone()),
            llm_port,
            tool_catalog,
            prompt_assembly,
            session_workspace_tools,
            session_store_worker: SessionStoreWorker::new(runtime_event_notifier.clone()),
            context_budget_worker: ContextBudgetWorker::new(runtime_event_notifier.clone())
                .map_err(|error| error.to_string())?,
            runtime_event_notifier,
            effect_scope,
            runtime_wake_effect: None,
            llm_port_effect: Some(llm_port_effect),
            tool_catalog_effect: Some(tool_catalog_effect),
            prompt_assembly_effect: Some(prompt_assembly_effect),
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
        self.reset_after_clear_with_native_mount_check(options, || Ok(()))
    }

    fn reset_after_clear_with_native_mount_check(
        &mut self,
        options: &AppRuntimeOptions,
        native_mount_check: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let replaced = [
            CapabilityKey::from("llm_port"),
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
        self.session_workspace_tools = ToolExecutorRegistry::new();
        let current_prompt_assembly = self.prompt_assembly.manager_snapshot();
        self.prompt_assembly.deactivate();
        self.llm_port.deactivate();
        if let Some(effect_id) = self.prompt_assembly_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if let Some(effect_id) = self.tool_catalog_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if let Some(effect_id) = self.llm_port_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if !failures.is_empty() {
            return Err(failures.join("; "));
        }

        let fresh_llm_port = LlmPort::new();
        let fresh_provider_registrations = fresh_llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let fresh_llm_port_effect =
            register_llm_port_effect(&self.effect_scope, fresh_provider_registrations)?;
        let (fresh_tool_catalog, fresh_tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
        )
        .map_err(|error| {
            let _ = self.effect_scope.dispose_effect(fresh_llm_port_effect);
            error.to_string()
        })?;
        let fresh_tool_catalog_effect =
            match register_tool_catalog_effect(&self.effect_scope, fresh_tool_registration) {
                Ok(effect_id) => effect_id,
                Err(error) => {
                    let _ = self.effect_scope.dispose_effect(fresh_llm_port_effect);
                    return Err(error);
                }
            };
        let prompt_assembly_tool_definitions = fresh_tool_catalog.definitions();
        let (fresh_prompt_assembly, fresh_prompt_registration) =
            match PromptAssembly::adopt_manager("workspace-prompt", current_prompt_assembly) {
                Ok(prompt_assembly) => prompt_assembly,
                Err(error) => {
                    let _ = self.effect_scope.dispose_effect(fresh_tool_catalog_effect);
                    let _ = self.effect_scope.dispose_effect(fresh_llm_port_effect);
                    return Err(error.to_string());
                }
            };
        let fresh_prompt_assembly_effect =
            match register_prompt_assembly_effect(&self.effect_scope, fresh_prompt_registration) {
                Ok(effect_id) => effect_id,
                Err(error) => {
                    let _ = self.effect_scope.dispose_effect(fresh_tool_catalog_effect);
                    let _ = self.effect_scope.dispose_effect(fresh_llm_port_effect);
                    return Err(error);
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
                self.runtime_event_notifier.clone(),
                fresh_llm_port.clone(),
            )
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = self
                    .effect_scope
                    .dispose_effect(fresh_prompt_assembly_effect);
                let _ = self.effect_scope.dispose_effect(fresh_tool_catalog_effect);
                let _ = self.effect_scope.dispose_effect(fresh_llm_port_effect);
                return Err(error);
            }
        };
        self.agent_runtime = fresh_agent_runtime;
        self.llm_port = fresh_llm_port;
        self.tool_catalog = fresh_tool_catalog;
        self.prompt_assembly = fresh_prompt_assembly;
        self.tool_catalog_effect = Some(fresh_tool_catalog_effect);
        self.llm_port_effect = Some(fresh_llm_port_effect);
        self.prompt_assembly_effect = Some(fresh_prompt_assembly_effect);
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
        for key in [
            "llm_port",
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
        if let Err(error) = self.model_refresh.shutdown() {
            failures.push(error);
        }
        self.session_workspace_tools = ToolExecutorRegistry::new();
        self.prompt_assembly.deactivate();
        self.llm_port.deactivate();
        if let Some(effect_id) = self.prompt_assembly_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if let Some(effect_id) = self.tool_catalog_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if let Some(effect_id) = self.llm_port_effect.take()
            && let Some(error) = self.effect_scope.dispose_effect(effect_id).error_message()
        {
            failures.push(error);
        }
        if let Some(error) = self.effect_scope.dispose().error_message() {
            failures.push(error);
        }
        if let Err(error) = self.context_budget_worker.shutdown() {
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

fn register_tool_catalog_effect(
    effect_scope: &EffectScope,
    mut registration: ToolRegistration,
) -> Result<EffectId, String> {
    effect_scope
        .register("workspace-tools", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn register_llm_port_effect(
    effect_scope: &EffectScope,
    mut registrations: ProviderRegistrations,
) -> Result<EffectId, String> {
    effect_scope
        .register("llm-providers", move || {
            registrations.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())
}

fn register_prompt_assembly_effect(
    effect_scope: &EffectScope,
    mut registration: PromptRegistration,
) -> Result<EffectId, String> {
    effect_scope
        .register("prompt-assembly", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())
}

impl Drop for RuntimeComponents {
    fn drop(&mut self) {
        let _ = self.shutdown(None);
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn failed_tool_catalog_remount_keeps_capabilities_removed_and_effects_reverted() {
        let options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&options).expect("runtime components should initialize");
        assert!(!components.tool_catalog.definitions().is_empty());
        assert_eq!(components.llm_port.inspection_snapshot().len(), 1);

        let report = components.effect_scope.dispose();
        assert!(report.failures.is_empty());
        assert!(components.tool_catalog.definitions().is_empty());
        assert!(components.llm_port.inspection_snapshot().is_empty());

        let error = components
            .reset_after_clear(&options)
            .expect_err("disposed effect scope must reject the replacement registration");

        assert_eq!(error, "effect scope is already disposed");
        assert!(components.tool_catalog.definitions().is_empty());
        assert_eq!(
            components
                .session_workspace_tools
                .definitions()
                .definitions()
                .count(),
            0
        );
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("tool_catalog"))
        );
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Pending)
        );
        assert!(components.prompt_assembly.manager_snapshot().is_none());
        assert!(components.prompt_assembly.inspection_snapshot().is_empty());
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("llm_port"))
        );
    }

    #[test]
    fn shutdown_clears_prompt_and_tool_generations() {
        let options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&options).expect("runtime components should initialize");

        components
            .shutdown(None)
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
    fn reset_rebinds_the_current_live_prompt_manager() {
        let options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&options).expect("runtime components should initialize");
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
        let options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let mut components =
            RuntimeComponents::new(&options).expect("runtime components should initialize");

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
        let options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&options).expect("runtime components should initialize");
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
}
