use std::{num::NonZeroU32, sync::Arc};

#[cfg(test)]
use std::sync::Mutex;

use conversation_runtime::{ModelRefreshWorker, RuntimeEventBinding, RuntimeEventNotifier};
use tool_runtime::ToolExecutorRegistry;

use super::{
    AppRuntimeOptions,
    agent::{AgentRuntimePort, NativeAgentRuntime, NativeAgentRuntimeMount},
    context::{
        ApprovalProviderCapability, CapabilityLease, ComponentActivationContext, LlmPortCapability,
        ModelCatalogCapability, PermissionPolicyCapability, PromptAssemblyCapability,
        RuntimeCapability, RuntimeContextError, RuntimeEventStreamCapability,
        RuntimeWakeCapability, SessionPersistenceCapability, ToolCatalogCapability,
    },
    context_budget_worker::ContextBudgetWorker,
    effect_scope::{EffectScope, EffectScopeSnapshot},
    lifecycle::CapabilityKey,
    lifecycle_executor::{
        ComponentActivationOutcome, ComponentLifecycleCallbacks, ComponentLifecycleExecutor,
        ComponentLifecycleMode, LifecycleExecutionError,
    },
    llm_port::{LlmPort, ProviderRegistrations},
    permission_policy::{
        ApprovalProviderRegistration, InteractiveApprovalProviderFactory, PermissionPolicy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    },
    plugin::{
        DesiredPluginComposition, PluginCatalogError, PluginComposition, PluginCompositionLoader,
        PluginDescriptor, PluginDescriptorBuilder, PluginDescriptorSnapshot, PluginFactory,
        PluginFactoryCatalog, PluginReloadPolicy, PluginTrust, PluginTypeId,
        PreparedPluginReconciliation,
    },
    prompt_assembly::{PromptAssembly, PromptRegistration},
    session_port::{SessionBackendRegistration, SessionBackendViews, SessionPortHost},
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

const NATIVE_AGENT_RUNTIME_COMPONENT: &str = "native_agent_runtime";
const MODEL_REFRESH_COMPONENT: &str = "model_refresh";
const CONTEXT_BUDGET_COMPONENT: &str = "context_budget";
const UI_RUNTIME_BRIDGE_COMPONENT: &str = "ui_runtime_bridge";

const APPROVAL_PROVIDER_PLUGIN: &str = "terminal-approval-provider";
const LLM_PORT_PLUGIN: &str = "openai-compatible-provider-catalog";
const RUNTIME_EVENT_STREAM_PLUGIN: &str = "runtime-event-stream";
const RUNTIME_WAKE_PLUGIN: &str = "runtime-wake-slot";
const SESSION_PERSISTENCE_PLUGIN: &str = "session-persistence";
const TOOL_CATALOG_PLUGIN: &str = "workspace-tools";
const PERMISSION_POLICY_PLUGIN: &str = "permission-policy";
const PROMPT_ASSEMBLY_PLUGIN: &str = "prompt-assembly";
const NATIVE_AGENT_RUNTIME_PLUGIN: &str = "native-agent-loop";
const MODEL_REFRESH_PLUGIN: &str = "model-refresh";
const CONTEXT_BUDGET_PLUGIN: &str = "context-budget";
const UI_RUNTIME_BRIDGE_PLUGIN: &str = "terminal-ui-runtime-adapter";

type RuntimePluginActivation = for<'a> fn(
    &mut RuntimeComponents,
    &EffectScope,
    &mut ComponentActivationContext<'a>,
    ComponentLifecycleMode,
) -> Result<ComponentActivationOutcome, String>;
type RuntimePluginQuiescence =
    fn(&mut RuntimeComponents, ComponentLifecycleMode) -> Result<(), String>;

#[derive(Clone, Copy)]
struct RuntimePluginImplementation {
    activate: RuntimePluginActivation,
    quiesce: RuntimePluginQuiescence,
}

fn builtin_plugin_type(type_id: &'static str) -> PluginTypeId {
    PluginTypeId::try_new(type_id).expect("builtin plugin type id must be valid")
}

fn builtin_descriptor(
    type_id: &'static str,
    display_name: &'static str,
) -> PluginDescriptorBuilder {
    PluginDescriptor::builder(
        builtin_plugin_type(type_id),
        display_name,
        NonZeroU32::MIN,
        PluginReloadPolicy::Replace,
        PluginTrust::Builtin,
    )
}

fn desired_builtin(
    component_id: &'static str,
    plugin_type: &'static str,
) -> (&'static str, PluginTypeId) {
    (component_id, builtin_plugin_type(plugin_type))
}

fn builtin_desired_composition() -> Result<DesiredPluginComposition, PluginCatalogError> {
    DesiredPluginComposition::try_new([
        desired_builtin(APPROVAL_PROVIDER.component_id, APPROVAL_PROVIDER_PLUGIN),
        desired_builtin(LLM_PORT.component_id, LLM_PORT_PLUGIN),
        desired_builtin(
            RUNTIME_EVENT_STREAM.component_id,
            RUNTIME_EVENT_STREAM_PLUGIN,
        ),
        desired_builtin(RUNTIME_WAKE.component_id, RUNTIME_WAKE_PLUGIN),
        desired_builtin(SESSION_PERSISTENCE.component_id, SESSION_PERSISTENCE_PLUGIN),
        desired_builtin(TOOL_CATALOG.component_id, TOOL_CATALOG_PLUGIN),
        desired_builtin(PERMISSION_POLICY.component_id, PERMISSION_POLICY_PLUGIN),
        desired_builtin(PROMPT_ASSEMBLY.component_id, PROMPT_ASSEMBLY_PLUGIN),
        desired_builtin(NATIVE_AGENT_RUNTIME_COMPONENT, NATIVE_AGENT_RUNTIME_PLUGIN),
        desired_builtin(MODEL_REFRESH_COMPONENT, MODEL_REFRESH_PLUGIN),
        desired_builtin(CONTEXT_BUDGET_COMPONENT, CONTEXT_BUDGET_PLUGIN),
        desired_builtin(UI_RUNTIME_BRIDGE_COMPONENT, UI_RUNTIME_BRIDGE_PLUGIN),
    ])
}

fn runtime_plugin_factory(
    descriptor: PluginDescriptorBuilder,
    activate: RuntimePluginActivation,
    quiesce: RuntimePluginQuiescence,
) -> PluginFactory<RuntimePluginImplementation> {
    let implementation = RuntimePluginImplementation { activate, quiesce };
    PluginFactory::new(
        descriptor
            .build()
            .expect("builtin plugin descriptor must be valid"),
        move || Ok(implementation),
    )
}

fn builtin_plugin_catalog()
-> Result<PluginFactoryCatalog<RuntimePluginImplementation>, PluginCatalogError> {
    PluginFactoryCatalog::try_new([
        runtime_plugin_factory(
            builtin_descriptor(APPROVAL_PROVIDER_PLUGIN, "Terminal approval provider")
                .provides(APPROVAL_PROVIDER.capability),
            RuntimeComponents::activate_approval_provider,
            RuntimeComponents::quiesce_noop,
        ),
        runtime_plugin_factory(
            builtin_descriptor(LLM_PORT_PLUGIN, "OpenAI-compatible provider catalog")
                .provides(LLM_PORT.capability)
                .provides(MODEL_CATALOG.capability),
            RuntimeComponents::activate_llm_port,
            RuntimeComponents::quiesce_llm_port,
        ),
        runtime_plugin_factory(
            builtin_descriptor(RUNTIME_EVENT_STREAM_PLUGIN, "Runtime event stream")
                .provides(RUNTIME_EVENT_STREAM.capability),
            RuntimeComponents::activate_runtime_event_stream,
            RuntimeComponents::quiesce_noop,
        ),
        runtime_plugin_factory(
            builtin_descriptor(RUNTIME_WAKE_PLUGIN, "Runtime wake slot")
                .provides(RUNTIME_WAKE.capability),
            RuntimeComponents::activate_runtime_wake,
            RuntimeComponents::quiesce_noop,
        ),
        runtime_plugin_factory(
            builtin_descriptor(SESSION_PERSISTENCE_PLUGIN, "Session persistence")
                .requires(RUNTIME_EVENT_STREAM.capability)
                .provides(SESSION_PERSISTENCE.capability),
            RuntimeComponents::activate_session_persistence,
            RuntimeComponents::quiesce_session_persistence,
        ),
        runtime_plugin_factory(
            builtin_descriptor(TOOL_CATALOG_PLUGIN, "Workspace tools")
                .provides(TOOL_CATALOG.capability),
            RuntimeComponents::activate_tool_catalog,
            RuntimeComponents::quiesce_tool_catalog,
        ),
        runtime_plugin_factory(
            builtin_descriptor(PERMISSION_POLICY_PLUGIN, "Permission policy")
                .requires(APPROVAL_PROVIDER.capability)
                .requires(RUNTIME_EVENT_STREAM.capability)
                .provides(PERMISSION_POLICY.capability),
            RuntimeComponents::activate_permission_policy,
            RuntimeComponents::quiesce_permission_policy,
        ),
        runtime_plugin_factory(
            builtin_descriptor(PROMPT_ASSEMBLY_PLUGIN, "Prompt assembly")
                .requires(TOOL_CATALOG.capability)
                .observes(SESSION_PERSISTENCE.capability)
                .provides(PROMPT_ASSEMBLY.capability),
            RuntimeComponents::activate_prompt_assembly,
            RuntimeComponents::quiesce_prompt_assembly,
        ),
        runtime_plugin_factory(
            builtin_descriptor(NATIVE_AGENT_RUNTIME_PLUGIN, "Native agent loop")
                .requires(RUNTIME_EVENT_STREAM.capability)
                .requires(LLM_PORT.capability)
                .requires(MODEL_CATALOG.capability)
                .requires(PERMISSION_POLICY.capability)
                .requires(PROMPT_ASSEMBLY.capability)
                .requires(TOOL_CATALOG.capability)
                .observes(SESSION_PERSISTENCE.capability),
            RuntimeComponents::activate_native_agent_runtime,
            RuntimeComponents::quiesce_native_agent_runtime,
        ),
        runtime_plugin_factory(
            builtin_descriptor(MODEL_REFRESH_PLUGIN, "Model refresh")
                .requires(RUNTIME_EVENT_STREAM.capability)
                .requires(LLM_PORT.capability)
                .requires(MODEL_CATALOG.capability),
            RuntimeComponents::activate_model_refresh,
            RuntimeComponents::quiesce_model_refresh,
        ),
        runtime_plugin_factory(
            builtin_descriptor(CONTEXT_BUDGET_PLUGIN, "Context budget")
                .requires(RUNTIME_EVENT_STREAM.capability)
                .requires(LLM_PORT.capability)
                .requires(MODEL_CATALOG.capability)
                .requires(PROMPT_ASSEMBLY.capability)
                .requires(TOOL_CATALOG.capability),
            RuntimeComponents::activate_context_budget,
            RuntimeComponents::quiesce_context_budget,
        ),
        runtime_plugin_factory(
            builtin_descriptor(UI_RUNTIME_BRIDGE_PLUGIN, "Terminal UI runtime adapter")
                .requires(RUNTIME_EVENT_STREAM.capability)
                .requires(RUNTIME_WAKE.capability),
            RuntimeComponents::activate_ui_runtime_bridge,
            RuntimeComponents::quiesce_noop,
        ),
    ])
}

#[cfg(test)]
fn builtin_plugin_composition()
-> Result<PluginComposition<RuntimePluginImplementation>, PluginCatalogError> {
    let desired = builtin_desired_composition()?;
    PluginCompositionLoader::try_new(builtin_plugin_catalog()?, desired)?.prepare_startup()
}

#[derive(Default)]
struct ComponentActivationStaging {
    approval_registration: Option<ApprovalProviderRegistration>,
    provider_registrations: Option<ProviderRegistrations>,
    tool_registration: Option<ToolRegistration>,
    prompt_registration: Option<PromptRegistration>,
    session_backend_registration: Option<SessionBackendRegistration>,
    runtime_wake: Option<RuntimeWake>,
    is_session_backend_replacement: bool,
}

struct PreparedPluginCommit {
    desired: DesiredPluginComposition,
    reconciliation: PreparedPluginReconciliation<RuntimePluginImplementation>,
}

/// `RuntimeComponents` 是 coordinator 的长期 runtime owner。
///
/// 它把 active Agent adapter、host workers、tool view、notifier 和 lifecycle graph 放在
/// 同一所有权边界，让 reset/shutdown 不再依赖 coordinator 手工枚举底层 conversation
/// resources。
pub(super) struct RuntimeComponents {
    agent_runtime: Box<dyn AgentRuntimePort>,
    pub(super) model_refresh: ModelRefreshWorker,
    llm_port: LlmPort,
    permission_policy: PermissionPolicy,
    permission_provider_id: String,
    tool_catalog: ToolCatalog,
    prompt_assembly: PromptAssembly,
    pub(super) session_workspace_tools: ToolExecutorRegistry,
    pub(super) session_port: Option<SessionPortHost>,
    session_backend_views: Option<SessionBackendViews>,
    pub(super) session_store_worker: SessionStoreWorker,
    pub(super) context_budget_worker: ContextBudgetWorker,
    runtime_event_notifier: RuntimeEventNotifier,
    activation_staging: ComponentActivationStaging,
    plugin_loader: PluginCompositionLoader<RuntimePluginImplementation>,
    plugins: PluginComposition<RuntimePluginImplementation>,
    prepared_plugin_commit: Option<PreparedPluginCommit>,
    #[cfg(test)]
    plugin_transaction_trace: Option<Arc<Mutex<Vec<String>>>>,
    pub(super) lifecycle: ComponentLifecycleExecutor,
    is_shutdown: bool,
}

fn boxed_native_agent_runtime(
    mount: NativeAgentRuntimeMount<'_>,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    NativeAgentRuntime::new(mount).map(|runtime| Box::new(runtime) as Box<dyn AgentRuntimePort>)
}

impl RuntimeComponents {
    pub(super) fn agent_port(&self) -> &dyn AgentRuntimePort {
        &*self.agent_runtime
    }

    pub(super) fn agent_port_mut(&mut self) -> &mut dyn AgentRuntimePort {
        &mut *self.agent_runtime
    }

    #[cfg(test)]
    pub(super) fn agent_test_harness(&mut self) -> &mut dyn super::agent::AgentRuntimeTestHarness {
        self.agent_runtime
            .test_harness()
            .expect("runtime test requires an Agent fixture harness")
    }

    #[cfg(test)]
    pub(super) fn agent_test_harness_ref(&self) -> &dyn super::agent::AgentRuntimeTestHarness {
        self.agent_runtime
            .test_harness_ref()
            .expect("runtime test requires an Agent fixture harness")
    }

    pub(super) fn new(options: &mut AppRuntimeOptions) -> Result<Self, String> {
        let desired_plugins = builtin_desired_composition().map_err(|error| error.to_string())?;
        let plugin_catalog = builtin_plugin_catalog().map_err(|error| error.to_string())?;
        let plugin_loader = PluginCompositionLoader::try_new(plugin_catalog, desired_plugins)
            .map_err(|error| error.to_string())?;
        let plugins = plugin_loader
            .prepare_startup()
            .map_err(|error| error.to_string())?;
        let runtime_event_notifier = RuntimeEventNotifier::default();
        let permission_policy = PermissionPolicy::new();
        let approval_registration = permission_policy
            .register(
                "terminal-runtime",
                TERMINAL_APPROVAL_PROVIDER_ID,
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .map_err(|error| error.to_string())?;
        let llm_port = LlmPort::new();
        let provider_registrations = llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let (tool_catalog, tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
        )
        .map_err(|error| error.to_string())?;
        let (prompt_assembly, prompt_registration) = PromptAssembly::adopt_manager(
            "workspace-prompt",
            options.initial_prompt_assembly.clone(),
        )
        .map_err(|error| error.to_string())?;
        let (session_port, session_backend_views, session_backend_registration) =
            mount_session_backend(options.session_store.take())?;
        let prompt_assembly_snapshot = prompt_assembly.session_snapshot();
        let prompt_assembly_tool_definitions = tool_catalog.definitions();
        let session_workspace_tools =
            session_tools_for_manager(&tool_catalog, prompt_assembly_snapshot.manager.as_ref());
        // Consumer 在 activation callback 中从 typed Context 换入 live generation；
        // bootstrap placeholder 不得提前持有 provider 的 raw notifier。
        let session_store_worker = SessionStoreWorker::new(RuntimeEventNotifier::default());
        let context_budget_worker = ContextBudgetWorker::new(RuntimeEventNotifier::default())
            .map_err(|error| error.to_string())?;
        let agent_runtime = boxed_native_agent_runtime(NativeAgentRuntimeMount {
            options,
            session_workspace_tools: session_workspace_tools.clone(),
            prompt_assembly_tool_definitions,
            prompt_assembly: prompt_assembly_snapshot,
            session_port: session_backend_views
                .as_ref()
                .map(|views| Arc::clone(&views.port)),
            llm_port: llm_port.clone(),
            permission_policy: permission_policy.clone(),
            permission_provider_id: TERMINAL_APPROVAL_PROVIDER_ID.to_string(),
        })?;
        let model_refresh = ModelRefreshWorker::new(RuntimeEventNotifier::default());
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
            activation_staging: ComponentActivationStaging {
                approval_registration: Some(approval_registration),
                provider_registrations: Some(provider_registrations),
                tool_registration: Some(tool_registration),
                prompt_registration: Some(prompt_registration),
                session_backend_registration,
                runtime_wake: None,
                is_session_backend_replacement: false,
            },
            plugin_loader,
            plugins,
            prepared_plugin_commit: None,
            #[cfg(test)]
            plugin_transaction_trace: None,
            lifecycle: ComponentLifecycleExecutor::default(),
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
        debug_assert_eq!(
            has_session_backend,
            self.activation_staging
                .session_backend_registration
                .is_some()
        );
        let definitions = self.plugins.definitions();
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.reconcile_definitions(
                definitions,
                components,
                ComponentLifecycleMode::Initial,
            )
        })
        .map_err(|error| error.to_string())
    }

    /// 在旧 component effects 完整退役后，原子切换 prepared plugin 与 desired definitions。
    ///
    /// `PluginFactoryCatalog::prepare_reconciliation` 只产生尚未 publication 的 fresh instances；
    /// lifecycle executor 的 commit hook 在 graph preflight、old cleanup 与 graph commit 全部
    /// 成功后才发布它。任何 pre-commit error 都只会 drop fresh composition，保留当前 authority。
    #[allow(dead_code)]
    fn reconcile_plugin_composition(
        &mut self,
        desired: DesiredPluginComposition,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let reconciliation = self
            .plugin_loader
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        self.commit_prepared_plugin_reconciliation(desired, reconciliation, mode)
    }

    #[cfg(test)]
    fn reconcile_plugin_composition_with_catalog(
        &mut self,
        catalog: &PluginFactoryCatalog<RuntimePluginImplementation>,
        desired: DesiredPluginComposition,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let reconciliation = catalog
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        self.commit_prepared_plugin_reconciliation(desired, reconciliation, mode)
    }

    fn commit_prepared_plugin_reconciliation(
        &mut self,
        desired: DesiredPluginComposition,
        reconciliation: PreparedPluginReconciliation<RuntimePluginImplementation>,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let definitions = reconciliation.definitions();
        debug_assert!(self.prepared_plugin_commit.is_none());
        self.prepared_plugin_commit = Some(PreparedPluginCommit {
            desired,
            reconciliation,
        });
        let result = self
            .with_lifecycle(|lifecycle, components| {
                lifecycle.reconcile_definitions_with_commit(definitions, components, mode)
            })
            .map_err(|error| format!("{error:?}"));
        if result.is_err() {
            // 未 publication 的 fresh instances 在所有 pre-commit error 上由 transaction owner
            // 逆序释放；旧 plugin authority 与旧 graph 保持不变。
            self.prepared_plugin_commit.take();
        }
        result
    }

    pub(super) fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let capability = CapabilityKey::from(RUNTIME_WAKE.capability);
        let replacements = self
            .lifecycle
            .has_capability_generation(&capability)
            .then_some((RUNTIME_WAKE.component_id, capability));
        self.lifecycle
            .validate_reconfiguration(replacements, [RUNTIME_WAKE.component_id])
            .map_err(|error| error.to_string())?;
        self.remove_runtime_wake()?;
        self.activation_staging.runtime_wake = Some(wake);
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [RUNTIME_WAKE.component_id],
                components,
                ComponentLifecycleMode::WakeBinding,
            )
        })
        .map_err(|error| error.to_string())
    }

    pub(super) fn remove_runtime_wake(&mut self) -> Result<(), String> {
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.deactivate_components(
                [RUNTIME_WAKE.component_id],
                components,
                ComponentLifecycleMode::WakeBinding,
            )
        })
        .map_err(|error| error.to_string())
    }

    /// 返回 composition root 下的 active component scope；root identity 不进入诊断投影。
    pub(super) fn effect_scope_snapshots(&self) -> Vec<EffectScopeSnapshot> {
        self.lifecycle.scope_snapshots()
    }

    /// 返回按 component id 排序的 builtin plugin descriptor projection。
    pub(super) fn plugin_descriptor_snapshots(&self) -> Vec<PluginDescriptorSnapshot> {
        self.plugins.descriptor_snapshots()
    }

    pub(super) fn require<C>(&self) -> Result<CapabilityLease<C>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        self.lifecycle.require::<C>()
    }

    pub(super) fn optional<C>(&self) -> Result<Option<CapabilityLease<C>>, RuntimeContextError>
    where
        C: RuntimeCapability + 'static,
    {
        self.lifecycle.optional::<C>()
    }

    pub(super) fn validate_context_alignment(&self) -> Result<(), String> {
        let graph = self
            .lifecycle
            .capabilities()
            .into_iter()
            .map(|snapshot| {
                (
                    snapshot.key,
                    snapshot.provider_component,
                    snapshot.generation,
                )
            })
            .collect::<Vec<_>>();
        let context = self
            .lifecycle
            .context_snapshots()
            .into_iter()
            .map(|snapshot| {
                (
                    snapshot.key,
                    snapshot.provider_component,
                    snapshot.generation,
                )
            })
            .collect::<Vec<_>>();
        if graph == context {
            Ok(())
        } else {
            Err("runtime capability context does not match lifecycle graph".to_string())
        }
    }

    pub(super) fn notify_runtime_event(&self) {
        match self.require::<RuntimeEventStreamCapability>() {
            Ok(notifier) => notifier.notify(),
            Err(RuntimeContextError::MissingCapability { .. }) => {}
            Err(RuntimeContextError::CapabilityTypeMismatch { .. }) => {
                panic!("RuntimeEventStream capability marker must match its registered value")
            }
            Err(RuntimeContextError::DependencyRetentionRejected { .. }) => {
                unreachable!("host lookup does not retain a component dependency")
            }
        }
    }

    fn with_lifecycle<T>(
        &mut self,
        operation: impl FnOnce(
            &mut ComponentLifecycleExecutor,
            &mut Self,
        ) -> Result<T, LifecycleExecutionError>,
    ) -> Result<T, LifecycleExecutionError> {
        let mut lifecycle = std::mem::take(&mut self.lifecycle);
        let result = operation(&mut lifecycle, self);
        self.lifecycle = lifecycle;
        result
    }

    #[cfg(test)]
    fn record_plugin_transaction_event(&self, event: impl Into<String>) {
        if let Some(trace) = &self.plugin_transaction_trace {
            trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event.into());
        }
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
        self.lifecycle
            .validate_reconfiguration(
                [(
                    SESSION_PERSISTENCE.component_id,
                    CapabilityKey::from(SESSION_PERSISTENCE.capability),
                )],
                [
                    SESSION_PERSISTENCE.component_id,
                    NATIVE_AGENT_RUNTIME_COMPONENT,
                ],
            )
            .map_err(|error| error.to_string())?;
        self.activation_staging.is_session_backend_replacement = true;
        if let Err(error) = self.with_lifecycle(|lifecycle, components| {
            lifecycle.deactivate_components(
                [
                    NATIVE_AGENT_RUNTIME_COMPONENT,
                    SESSION_PERSISTENCE.component_id,
                ],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        }) {
            let cleanup_error = error.to_string();
            self.activation_staging.is_session_backend_replacement = false;
            self.restore_ephemeral_session_consumers(options)
                .map_err(|fallback_error| format!("{cleanup_error}; {fallback_error}"))?;
            return Err(cleanup_error);
        }

        let (fresh_session_port, fresh_views, fresh_registration) =
            match mount_session_backend(Some(store)) {
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
                drop(fresh_registration);
                self.restore_ephemeral_session_consumers(options)
                    .map_err(|fallback_error| format!("{error}; {fallback_error}"))?;
                return Err(error);
            }
        };

        self.session_store_worker = SessionStoreWorker::default();
        self.agent_runtime = fresh_agent_runtime;
        self.session_port = fresh_session_port;
        self.session_backend_views = Some(fresh_views);
        self.activation_staging.session_backend_registration = fresh_registration;
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [
                    SESSION_PERSISTENCE.component_id,
                    NATIVE_AGENT_RUNTIME_COMPONENT,
                ],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())
    }

    fn fresh_native_agent_runtime(
        &self,
        options: &AppRuntimeOptions,
        session_port: Option<Arc<dyn session_store::SessionPort>>,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        boxed_native_agent_runtime(NativeAgentRuntimeMount {
            options,
            session_workspace_tools: self.session_workspace_tools.clone(),
            prompt_assembly_tool_definitions: self.tool_catalog.definitions(),
            prompt_assembly: self.prompt_assembly.session_snapshot(),
            session_port,
            llm_port: self.llm_port.clone(),
            permission_policy: self.permission_policy.clone(),
            permission_provider_id: self.permission_provider_id.clone(),
        })
    }

    fn restore_ephemeral_session_consumers(
        &mut self,
        options: &AppRuntimeOptions,
    ) -> Result<(), String> {
        let fresh_agent_runtime = self
            .fresh_native_agent_runtime(options, None)
            .map_err(|error| format!("restore ephemeral Agent after backend failure: {error}"))?;
        self.agent_runtime = fresh_agent_runtime;
        self.session_store_worker = SessionStoreWorker::default();
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [NATIVE_AGENT_RUNTIME_COMPONENT],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())
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
        self.lifecycle
            .validate_reconfiguration(
                replaced.map(|capability| {
                    (
                        capability.component_id,
                        CapabilityKey::from(capability.capability),
                    )
                }),
                [APPROVAL_PROVIDER.component_id],
            )
            .map_err(|error| error.to_string())?;
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.deactivate_components(
                [APPROVAL_PROVIDER.component_id],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())?;

        let provider_id = provider_id.into();
        let fresh_policy = PermissionPolicy::new();
        let fresh_registration = fresh_policy
            .register(owner, provider_id.clone(), factory)
            .map_err(|error| error.to_string())?;
        let fresh_agent_runtime = match native_mount_check().and_then(|()| {
            boxed_native_agent_runtime(NativeAgentRuntimeMount {
                options,
                session_workspace_tools: self.session_workspace_tools.clone(),
                prompt_assembly_tool_definitions: self.tool_catalog.definitions(),
                prompt_assembly: self.prompt_assembly.session_snapshot(),
                session_port: self
                    .session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                llm_port: self.llm_port.clone(),
                permission_policy: fresh_policy.clone(),
                permission_provider_id: provider_id.clone(),
            })
        }) {
            Ok(runtime) => runtime,
            Err(error) => {
                fresh_policy.deactivate();
                drop(fresh_registration);
                return Err(error);
            }
        };

        self.permission_policy = fresh_policy;
        self.permission_provider_id = provider_id;
        self.agent_runtime = fresh_agent_runtime;
        self.activation_staging.approval_registration = Some(fresh_registration);
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [APPROVAL_PROVIDER.component_id],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())
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
        self.lifecycle
            .validate_reconfiguration(
                replaced.map(|capability| {
                    (
                        capability.component_id,
                        CapabilityKey::from(capability.capability),
                    )
                }),
                [LLM_PORT.component_id, TOOL_CATALOG.component_id],
            )
            .map_err(|error| error.to_string())?;

        let current_prompt_assembly = self.prompt_assembly.manager_snapshot();
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.deactivate_components(
                [TOOL_CATALOG.component_id, LLM_PORT.component_id],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())?;

        let fresh_llm_port = LlmPort::new();
        let fresh_provider_registrations = fresh_llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let (fresh_tool_catalog, fresh_tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
        )
        .map_err(|error| error.to_string())?;
        let prompt_assembly_tool_definitions = fresh_tool_catalog.definitions();
        let (fresh_prompt_assembly, fresh_prompt_registration) =
            PromptAssembly::adopt_manager("workspace-prompt", current_prompt_assembly)
                .map_err(|error| error.to_string())?;
        let fresh_prompt_assembly_snapshot = fresh_prompt_assembly.session_snapshot();
        let session_workspace_tools = session_tools_for_manager(
            &fresh_tool_catalog,
            fresh_prompt_assembly_snapshot.manager.as_ref(),
        );
        // 旧 adapter 完全 quiescent 后才创建新 generation，避免 reset 期间存在两个
        // native worker path；构造失败时 capability 仍保持 removed，不发布半成品。
        let fresh_agent_runtime = native_mount_check().and_then(|()| {
            boxed_native_agent_runtime(NativeAgentRuntimeMount {
                options,
                session_workspace_tools: session_workspace_tools.clone(),
                prompt_assembly_tool_definitions,
                prompt_assembly: fresh_prompt_assembly_snapshot,
                session_port: self
                    .session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                llm_port: fresh_llm_port.clone(),
                permission_policy: self.permission_policy.clone(),
                permission_provider_id: self.permission_provider_id.clone(),
            })
        })?;
        self.agent_runtime = fresh_agent_runtime;
        self.llm_port = fresh_llm_port;
        self.tool_catalog = fresh_tool_catalog;
        self.prompt_assembly = fresh_prompt_assembly;
        self.session_workspace_tools = session_workspace_tools;
        self.activation_staging.provider_registrations = Some(fresh_provider_registrations);
        self.activation_staging.tool_registration = Some(fresh_tool_registration);
        self.activation_staging.prompt_registration = Some(fresh_prompt_registration);
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [LLM_PORT.component_id, TOOL_CATALOG.component_id],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())
    }

    fn activate_approval_provider(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let registration = self
            .activation_staging
            .approval_registration
            .take()
            .ok_or_else(|| "approval provider activation was not staged".to_string())?;
        register_permission_policy_effect(scope, registration)?;
        context
            .publish::<ApprovalProviderCapability>(self.permission_policy.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_llm_port(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let registrations = self
            .activation_staging
            .provider_registrations
            .take()
            .ok_or_else(|| "LLM provider activation was not staged".to_string())?;
        register_llm_port_effect(scope, registrations)?;
        context
            .publish::<LlmPortCapability>(self.llm_port.clone())
            .map_err(|error| error.to_string())?;
        context
            .publish::<ModelCatalogCapability>(self.llm_port.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_tool_catalog(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let registration = self
            .activation_staging
            .tool_registration
            .take()
            .ok_or_else(|| "tool catalog activation was not staged".to_string())?;
        register_tool_catalog_effect(scope, registration)?;
        context
            .publish::<ToolCatalogCapability>(self.tool_catalog.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_prompt_assembly(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let registration = self
            .activation_staging
            .prompt_registration
            .take()
            .ok_or_else(|| "prompt assembly activation was not staged".to_string())?;
        register_prompt_assembly_effect(scope, registration)?;
        context
            .publish::<PromptAssemblyCapability>(self.prompt_assembly.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_session_persistence(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.session_store_worker.shutdown()?;
        self.session_store_worker = SessionStoreWorker::new((*event_stream).clone());
        if let Some(registration) = self.activation_staging.session_backend_registration.take() {
            register_session_backend_effect(scope, registration)?;
        }
        if let Some(views) = self.session_backend_views.clone() {
            context
                .publish::<SessionPersistenceCapability>(views)
                .map_err(|error| error.to_string())?;
            Ok(ComponentActivationOutcome::PublishCapabilities)
        } else {
            Ok(ComponentActivationOutcome::Ready)
        }
    }

    fn activate_runtime_wake(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        if let Some(wake) = self.activation_staging.runtime_wake.take() {
            context
                .publish::<RuntimeWakeCapability>(wake)
                .map_err(|error| error.to_string())?;
            Ok(ComponentActivationOutcome::PublishCapabilities)
        } else {
            Ok(ComponentActivationOutcome::Ready)
        }
    }

    fn activate_permission_policy(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.permission_policy.bind_event_stream(event_stream);
        context
            .publish::<PermissionPolicyCapability>(self.permission_policy.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_runtime_event_stream(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        context
            .publish::<RuntimeEventStreamCapability>(self.runtime_event_notifier.clone())
            .map_err(|error| error.to_string())?;
        Ok(ComponentActivationOutcome::PublishCapabilities)
    }

    fn activate_ui_runtime_bridge(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        let wake = context
            .require::<RuntimeWakeCapability>()
            .map_err(|error| error.to_string())?;
        let binding = event_stream.bind_callback(move || wake.wake());
        register_runtime_wake_effect(scope, binding)?;
        Ok(ComponentActivationOutcome::Ready)
    }

    fn activate_native_agent_runtime(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.agent_runtime.activate(event_stream)?;
        Ok(ComponentActivationOutcome::Ready)
    }

    fn activate_model_refresh(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.model_refresh.shutdown()?;
        self.model_refresh = ModelRefreshWorker::new((*event_stream).clone());
        Ok(ComponentActivationOutcome::Ready)
    }

    fn activate_context_budget(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.context_budget_worker
            .rebind_event_notifier((*event_stream).clone())?;
        Ok(ComponentActivationOutcome::Ready)
    }

    fn quiesce_native_agent_runtime(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        match mode {
            ComponentLifecycleMode::Shutdown => self.agent_runtime.shutdown(),
            _ => self.agent_runtime.suspend(),
        }
        .map_err(|error| error.to_string())
    }

    fn quiesce_model_refresh(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        match mode {
            ComponentLifecycleMode::Shutdown => self.model_refresh.shutdown(),
            _ => self.model_refresh.reset_after_clear(),
        }
    }

    fn quiesce_context_budget(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        match mode {
            ComponentLifecycleMode::Shutdown => self.context_budget_worker.shutdown(),
            _ => {
                self.context_budget_worker.cancel_pending();
                Ok(())
            }
        }
    }

    fn quiesce_permission_policy(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        self.permission_policy.deactivate();
        Ok(())
    }

    fn quiesce_llm_port(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        self.llm_port.deactivate();
        Ok(())
    }

    fn quiesce_tool_catalog(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        self.session_workspace_tools = ToolExecutorRegistry::new();
        Ok(())
    }

    fn quiesce_prompt_assembly(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        self.prompt_assembly.deactivate();
        Ok(())
    }

    fn quiesce_session_persistence(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        let mut failures = Vec::new();
        if self.session_store_worker.is_running()
            && let Some(views) = self.session_backend_views.clone()
            && let Err(error) = self.session_store_worker.flush_all(views)
        {
            failures.push(error);
        }
        if let Err(error) = self.session_store_worker.shutdown() {
            failures.push(error);
        }
        if mode == ComponentLifecycleMode::Shutdown
            || self.activation_staging.is_session_backend_replacement
        {
            if let Some(session_port) = &self.session_port {
                session_port.deactivate();
            }
            self.session_backend_views = None;
            self.session_port = None;
            self.activation_staging.is_session_backend_replacement = false;
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    fn quiesce_noop(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        Ok(())
    }

    pub(super) fn shutdown(&mut self) -> Result<(), String> {
        if self.is_shutdown {
            return Ok(());
        }
        self.is_shutdown = true;
        let shutdown_error = self
            .with_lifecycle(|lifecycle, components| lifecycle.shutdown(components))
            .err()
            .map(|error| error.to_string());
        match shutdown_error {
            None => Ok(()),
            Some(error) => Err(error),
        }
    }
}

impl ComponentLifecycleCallbacks for RuntimeComponents {
    fn commit_authority(&mut self) {
        let prepared = self
            .prepared_plugin_commit
            .take()
            .expect("plugin authority commit must have a prepared composition");
        self.plugins.commit_reconciliation(prepared.reconciliation);
        self.plugin_loader.commit_desired(prepared.desired);
        #[cfg(test)]
        self.record_plugin_transaction_event("authority:commit");
    }

    fn activate_component(
        &mut self,
        component_id: &str,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let implementation = self
            .plugins
            .implementation(component_id)
            .copied()
            .ok_or_else(|| format!("component `{component_id}` has no prepared plugin instance"))?;
        #[cfg(test)]
        self.record_plugin_transaction_event(format!("activate:{component_id}"));
        (implementation.activate)(self, scope, context, mode)
    }

    fn quiesce_component(
        &mut self,
        component_id: &str,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let implementation = self
            .plugins
            .implementation(component_id)
            .copied()
            .ok_or_else(|| format!("component `{component_id}` has no prepared plugin instance"))?;
        #[cfg(test)]
        self.record_plugin_transaction_event(format!("quiesce:{component_id}"));
        (implementation.quiesce)(self, mode)
    }
}

fn register_tool_catalog_effect(
    scope: &EffectScope,
    mut registration: ToolRegistration,
) -> Result<(), String> {
    scope
        .register("tool_registrations", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn register_llm_port_effect(
    scope: &EffectScope,
    mut registrations: ProviderRegistrations,
) -> Result<(), String> {
    scope
        .register("provider_registrations", move || {
            registrations.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn register_permission_policy_effect(
    scope: &EffectScope,
    mut registration: ApprovalProviderRegistration,
) -> Result<(), String> {
    scope
        .register("approval_provider_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn register_prompt_assembly_effect(
    scope: &EffectScope,
    mut registration: PromptRegistration,
) -> Result<(), String> {
    scope
        .register("prompt_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

type MountedSessionBackend = (
    Option<SessionPortHost>,
    Option<SessionBackendViews>,
    Option<SessionBackendRegistration>,
);

fn mount_session_backend(
    store: Option<Arc<dyn session_store::SessionStore>>,
) -> Result<MountedSessionBackend, String> {
    let Some(store) = store else {
        return Ok((None, None, None));
    };
    let session_port = SessionPortHost::new();
    let registration = session_port
        .register("terminal-runtime", "configured-session-store", store)
        .map_err(|error| error.to_string())?;
    let views = match session_port.views() {
        Ok(views) => views,
        Err(error) => {
            session_port.deactivate();
            return Err(error.to_string());
        }
    };
    Ok((Some(session_port), Some(views), Some(registration)))
}

fn register_session_backend_effect(
    scope: &EffectScope,
    mut registration: SessionBackendRegistration,
) -> Result<(), String> {
    scope
        .register("backend_registration", move || {
            registration.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn register_runtime_wake_effect(
    scope: &EffectScope,
    mut binding: RuntimeEventBinding,
) -> Result<(), String> {
    scope
        .register("runtime_wake_binding", move || {
            binding.dispose();
            Ok(())
        })
        .map_err(|error| error.to_string())
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
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };

    use super::*;
    use crate::runtime::agent::{
        AgentCommand, AgentCommandReceipt, AgentContextBudgetSnapshot, AgentEvent, AgentRuntime,
        AgentRuntimeError, AgentSessionRestore,
    };
    use crate::runtime::lifecycle::{ComponentDefinition, ComponentState};
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

    fn agent_system_prompt(components: &RuntimeComponents) -> Option<String> {
        components
            .agent_port()
            .context_budget_snapshot()
            .items
            .iter()
            .find(|item| item.role() == Some(provider_protocol::Role::System))
            .map(provider_protocol::ConversationItem::text_content)
    }

    struct RecordingAgentRuntime {
        lifecycle_trace: Arc<Mutex<Vec<&'static str>>>,
        is_shutdown: bool,
    }

    impl RecordingAgentRuntime {
        fn new(lifecycle_trace: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                lifecycle_trace,
                is_shutdown: true,
            }
        }

        fn record(&self, event: &'static str) {
            self.lifecycle_trace
                .lock()
                .expect("recording Agent trace lock should not be poisoned")
                .push(event);
        }
    }

    impl AgentRuntime for RecordingAgentRuntime {
        fn dispatch(
            &mut self,
            _command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            if self.is_shutdown {
                Err(AgentRuntimeError::Disposed)
            } else {
                Err(AgentRuntimeError::CommandRejected(
                    "recording Agent does not accept commands".to_string(),
                ))
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            if !self.is_shutdown {
                self.record("shutdown");
                self.is_shutdown = true;
            }
            Ok(())
        }
    }

    impl AgentRuntimePort for RecordingAgentRuntime {
        fn activate(
            &mut self,
            _event_stream: CapabilityLease<RuntimeEventStreamCapability>,
        ) -> Result<(), String> {
            self.record("activate");
            self.is_shutdown = false;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            if !self.is_shutdown {
                self.record("suspend");
                self.is_shutdown = true;
            }
            Ok(())
        }

        fn is_busy(&self) -> bool {
            false
        }

        fn session_id(&self) -> Option<session_store::SessionId> {
            None
        }

        fn is_history_empty(&self) -> bool {
            true
        }

        fn is_idle_empty_session(&self) -> bool {
            !self.is_shutdown
        }

        fn truncate_after_user_turns(
            &mut self,
            _retained_user_turns: usize,
        ) -> Result<Option<(session_store::SessionId, String)>, String> {
            Ok(None)
        }

        fn context_budget_snapshot(&self) -> AgentContextBudgetSnapshot {
            AgentContextBudgetSnapshot {
                items: Arc::from([]),
                prompt_prelude: None,
                upstream_context_tokens: None,
                tool_definitions: Vec::new(),
            }
        }

        fn update_empty_session_configuration(
            &mut self,
            _prompt_assembly: crate::runtime::prompt_assembly::PromptAssemblySessionSnapshot,
            _session_workspace_tools: ToolExecutorRegistry,
        ) {
        }

        fn restore_session(&mut self, _restore: AgentSessionRestore) -> Result<(), String> {
            Ok(())
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    #[test]
    fn runtime_components_owns_alternate_agent_through_the_lifecycle_port() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [NATIVE_AGENT_RUNTIME_COMPONENT],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("old Agent owner should quiesce before replacement");
        components.agent_runtime =
            Box::new(RecordingAgentRuntime::new(Arc::clone(&lifecycle_trace)));
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.activate_components(
                    [NATIVE_AGENT_RUNTIME_COMPONENT],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("recording Agent should activate through the lifecycle port");
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("dependency removal should suspend the recording Agent");
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.activate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("dependency republication should reactivate the recording Agent");
        components
            .shutdown()
            .expect("shutdown should dispose the recording Agent");

        assert_eq!(
            *lifecycle_trace
                .lock()
                .expect("recording Agent trace lock should not be poisoned"),
            ["activate", "suspend", "activate", "shutdown"]
        );
    }

    #[test]
    fn runtime_components_source_keeps_agent_ownership_erased() {
        let source = include_str!("components.rs");
        let concrete_owner = ["agent_runtime: ", "NativeAgentRuntime"].concat();
        let concrete_constructor = ["NativeAgentRuntime", "::new("].concat();

        assert!(source.contains("agent_runtime: Box<dyn AgentRuntimePort>"));
        assert!(!source.contains(&concrete_owner));
        assert_eq!(source.matches(&concrete_constructor).count(), 1);
        for forbidden in [
            ["downcast", "_ref"].concat(),
            ["downcast", "_mut"].concat(),
            ["std::any", "::Any"].concat(),
            ["dyn", " Any"].concat(),
        ] {
            assert!(
                !source.contains(&forbidden),
                "Agent owner must not regain concrete access through {forbidden}"
            );
        }
        let concrete_binding = ["agent_runtime", ".bind_event_stream"].concat();
        assert!(!source.contains(&concrete_binding));
        assert!(source.contains("self.agent_runtime.activate(event_stream)?"));
        assert!(source.contains("self.agent_runtime.suspend()"));
        assert!(source.contains("self.agent_runtime.shutdown()"));
    }

    fn desired_with_runtime_event_plugin(plugin_type: &'static str) -> DesiredPluginComposition {
        desired_with_runtime_event_plugin_in_order(plugin_type, false)
    }

    fn desired_with_runtime_event_plugin_in_order(
        plugin_type: &'static str,
        reverse: bool,
    ) -> DesiredPluginComposition {
        let desired = builtin_desired_composition().expect("builtin desired state should validate");
        let mut entries = desired
            .iter()
            .map(|(component_id, current_type)| {
                let plugin_type = if component_id.as_str() == RUNTIME_EVENT_STREAM.component_id {
                    builtin_plugin_type(plugin_type)
                } else {
                    current_type.clone()
                };
                (component_id.as_str(), plugin_type)
            })
            .collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        DesiredPluginComposition::try_new(entries)
            .expect("replacement desired state should validate")
    }

    fn runtime_event_replacement_factory(
        plugin_type: &'static str,
        activate: RuntimePluginActivation,
    ) -> PluginFactory<RuntimePluginImplementation> {
        runtime_plugin_factory(
            builtin_descriptor(plugin_type, "Replacement runtime event stream")
                .provides(RUNTIME_EVENT_STREAM.capability),
            activate,
            RuntimeComponents::quiesce_noop,
        )
    }

    fn traced_runtime_event_replacement_factory(
        plugin_type: &'static str,
        trace: Arc<Mutex<Vec<String>>>,
    ) -> PluginFactory<RuntimePluginImplementation> {
        let descriptor = builtin_descriptor(plugin_type, "Traced runtime event stream")
            .provides(RUNTIME_EVENT_STREAM.capability)
            .build()
            .expect("traced descriptor should validate");
        PluginFactory::new(descriptor, move || {
            trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("prepare:{plugin_type}"));
            Ok(RuntimePluginImplementation {
                activate: RuntimeComponents::activate_runtime_event_stream,
                quiesce: RuntimeComponents::quiesce_noop,
            })
        })
    }

    fn reject_runtime_event_activation(
        _components: &mut RuntimeComponents,
        _scope: &EffectScope,
        _context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        Err("SENSITIVE_REPLACEMENT_ACTIVATION".to_string())
    }

    #[test]
    fn builtin_plugin_descriptors_define_the_exact_default_graph() {
        let composition =
            builtin_plugin_composition().expect("builtin plugin composition should prepare");
        let observed = composition.observed();

        assert_eq!(
            observed
                .iter()
                .map(|(component_id, plugin_type)| {
                    (component_id.as_str(), plugin_type.as_str())
                })
                .collect::<Vec<_>>(),
            vec![
                (APPROVAL_PROVIDER.component_id, APPROVAL_PROVIDER_PLUGIN),
                (CONTEXT_BUDGET_COMPONENT, CONTEXT_BUDGET_PLUGIN),
                (LLM_PORT.component_id, LLM_PORT_PLUGIN),
                (MODEL_REFRESH_COMPONENT, MODEL_REFRESH_PLUGIN),
                (NATIVE_AGENT_RUNTIME_COMPONENT, NATIVE_AGENT_RUNTIME_PLUGIN),
                (PERMISSION_POLICY.component_id, PERMISSION_POLICY_PLUGIN),
                (PROMPT_ASSEMBLY.component_id, PROMPT_ASSEMBLY_PLUGIN),
                (
                    RUNTIME_EVENT_STREAM.component_id,
                    RUNTIME_EVENT_STREAM_PLUGIN,
                ),
                (RUNTIME_WAKE.component_id, RUNTIME_WAKE_PLUGIN),
                (SESSION_PERSISTENCE.component_id, SESSION_PERSISTENCE_PLUGIN),
                (TOOL_CATALOG.component_id, TOOL_CATALOG_PLUGIN),
                (UI_RUNTIME_BRIDGE_COMPONENT, UI_RUNTIME_BRIDGE_PLUGIN),
            ]
        );

        assert_eq!(
            composition.definitions(),
            vec![
                ComponentDefinition::new(APPROVAL_PROVIDER.component_id)
                    .implemented_by(APPROVAL_PROVIDER_PLUGIN)
                    .provides(APPROVAL_PROVIDER.capability),
                ComponentDefinition::new(CONTEXT_BUDGET_COMPONENT)
                    .implemented_by(CONTEXT_BUDGET_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .requires(LLM_PORT.capability)
                    .requires(MODEL_CATALOG.capability)
                    .requires(PROMPT_ASSEMBLY.capability)
                    .requires(TOOL_CATALOG.capability),
                ComponentDefinition::new(LLM_PORT.component_id)
                    .implemented_by(LLM_PORT_PLUGIN)
                    .provides(LLM_PORT.capability)
                    .provides(MODEL_CATALOG.capability),
                ComponentDefinition::new(MODEL_REFRESH_COMPONENT)
                    .implemented_by(MODEL_REFRESH_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .requires(LLM_PORT.capability)
                    .requires(MODEL_CATALOG.capability),
                ComponentDefinition::new(NATIVE_AGENT_RUNTIME_COMPONENT)
                    .implemented_by(NATIVE_AGENT_RUNTIME_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .requires(LLM_PORT.capability)
                    .requires(MODEL_CATALOG.capability)
                    .requires(PERMISSION_POLICY.capability)
                    .requires(PROMPT_ASSEMBLY.capability)
                    .requires(TOOL_CATALOG.capability)
                    .observes(SESSION_PERSISTENCE.capability),
                ComponentDefinition::new(PERMISSION_POLICY.component_id)
                    .implemented_by(PERMISSION_POLICY_PLUGIN)
                    .requires(APPROVAL_PROVIDER.capability)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .provides(PERMISSION_POLICY.capability),
                ComponentDefinition::new(PROMPT_ASSEMBLY.component_id)
                    .implemented_by(PROMPT_ASSEMBLY_PLUGIN)
                    .requires(TOOL_CATALOG.capability)
                    .observes(SESSION_PERSISTENCE.capability)
                    .provides(PROMPT_ASSEMBLY.capability),
                ComponentDefinition::new(RUNTIME_EVENT_STREAM.component_id)
                    .implemented_by(RUNTIME_EVENT_STREAM_PLUGIN)
                    .provides(RUNTIME_EVENT_STREAM.capability),
                ComponentDefinition::new(RUNTIME_WAKE.component_id)
                    .implemented_by(RUNTIME_WAKE_PLUGIN)
                    .provides(RUNTIME_WAKE.capability),
                ComponentDefinition::new(SESSION_PERSISTENCE.component_id)
                    .implemented_by(SESSION_PERSISTENCE_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .provides(SESSION_PERSISTENCE.capability),
                ComponentDefinition::new(TOOL_CATALOG.component_id)
                    .implemented_by(TOOL_CATALOG_PLUGIN)
                    .provides(TOOL_CATALOG.capability),
                ComponentDefinition::new(UI_RUNTIME_BRIDGE_COMPONENT)
                    .implemented_by(UI_RUNTIME_BRIDGE_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .requires(RUNTIME_WAKE.capability),
            ]
        );
    }

    #[test]
    fn lifecycle_dispatch_rejects_a_component_without_a_prepared_plugin_instance() {
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        let error = ComponentLifecycleCallbacks::quiesce_component(
            &mut components,
            "unregistered_component",
            ComponentLifecycleMode::Reconfigure,
        )
        .expect_err("unknown component must not reach a lifecycle handler");

        assert_eq!(
            error,
            "component `unregistered_component` has no prepared plugin instance"
        );
    }

    #[test]
    fn plugin_replacement_switches_actual_authority_and_preserves_alignment() {
        const FRESH_PLUGIN: &str = "runtime-event-stream-v2";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_generation = components
            .lifecycle
            .capability(&CapabilityKey::from(RUNTIME_EVENT_STREAM.capability))
            .expect("runtime event stream should be active")
            .generation;
        let catalog = PluginFactoryCatalog::try_new([runtime_event_replacement_factory(
            FRESH_PLUGIN,
            RuntimeComponents::activate_runtime_event_stream,
        )])
        .expect("replacement catalog should validate");

        components
            .reconcile_plugin_composition_with_catalog(
                &catalog,
                desired_with_runtime_event_plugin(FRESH_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("replacement should commit and activate");

        let descriptor = components
            .plugin_descriptor_snapshots()
            .into_iter()
            .find(|snapshot| snapshot.component_id == RUNTIME_EVENT_STREAM.component_id)
            .expect("replacement descriptor should be visible");
        assert_eq!(descriptor.plugin_type, FRESH_PLUGIN);
        assert!(
            components
                .lifecycle
                .capability(&CapabilityKey::from(RUNTIME_EVENT_STREAM.capability))
                .expect("fresh runtime event stream should be active")
                .generation
                > old_generation
        );
        assert_eq!(
            components
                .plugin_loader
                .desired()
                .iter()
                .find(|(component_id, _)| {
                    component_id.as_str() == RUNTIME_EVENT_STREAM.component_id
                })
                .expect("committed desired input should contain runtime event stream")
                .1
                .as_str(),
            FRESH_PLUGIN
        );
        components
            .validate_context_alignment()
            .expect("plugin, graph, and typed Context should align");
    }

    #[test]
    fn plugin_transaction_trace_is_stable_for_reordered_desired_input() {
        fn run(reverse: bool) -> Vec<String> {
            const FRESH_PLUGIN: &str = "runtime-event-stream-traced";
            let trace = Arc::new(Mutex::new(Vec::new()));
            let mut options = AppRuntimeOptions::default();
            let mut components =
                RuntimeComponents::new(&mut options).expect("runtime components should initialize");
            components.plugin_transaction_trace = Some(Arc::clone(&trace));
            let catalog =
                PluginFactoryCatalog::try_new([traced_runtime_event_replacement_factory(
                    FRESH_PLUGIN,
                    Arc::clone(&trace),
                )])
                .expect("traced catalog should validate");

            components
                .reconcile_plugin_composition_with_catalog(
                    &catalog,
                    desired_with_runtime_event_plugin_in_order(FRESH_PLUGIN, reverse),
                    ComponentLifecycleMode::Reconfigure,
                )
                .expect("traced replacement should commit");
            let snapshot = trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            components.plugin_transaction_trace = None;
            snapshot
        }

        let forward = run(false);
        let reverse = run(true);
        assert_eq!(forward, reverse);
        let authority_index = forward
            .iter()
            .position(|event| event == "authority:commit")
            .expect("authority switch should be observable");
        assert!(
            forward[..authority_index]
                .iter()
                .all(|event| event.starts_with("prepare:") || event.starts_with("quiesce:"))
        );
        assert!(
            forward[authority_index + 1..]
                .iter()
                .all(|event| event.starts_with("activate:"))
        );
    }

    #[test]
    fn repeated_plugin_replacements_reject_a_stale_old_token() {
        use crate::runtime::lifecycle::ComponentGraphError;

        fn stale_token(
            components: &RuntimeComponents,
            component_id: &str,
        ) -> crate::runtime::lifecycle::DeactivationToken {
            let mut graph = components.lifecycle.graph().clone();
            graph
                .suspend(component_id)
                .expect("active component should produce a stale token")
                .deactivation_requests
                .into_iter()
                .find(|token| token.component_id() == component_id)
                .expect("component deactivation token should exist")
        }

        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let stale = stale_token(&components, RUNTIME_EVENT_STREAM.component_id);

        for plugin_type in ["runtime-event-stream-v2", "runtime-event-stream-v3"] {
            let catalog = PluginFactoryCatalog::try_new([runtime_event_replacement_factory(
                plugin_type,
                RuntimeComponents::activate_runtime_event_stream,
            )])
            .expect("replacement catalog should validate");
            components
                .reconcile_plugin_composition_with_catalog(
                    &catalog,
                    desired_with_runtime_event_plugin(plugin_type),
                    ComponentLifecycleMode::Reconfigure,
                )
                .expect("replacement should commit");
        }

        assert!(matches!(
            components.lifecycle.complete_deactivation(stale),
            Err(ComponentGraphError::StaleDeactivation { component_id, .. })
                if component_id == RUNTIME_EVENT_STREAM.component_id
        ));
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == RUNTIME_EVENT_STREAM.component_id)
                .expect("fresh descriptor should remain authoritative")
                .plugin_type,
            "runtime-event-stream-v3"
        );
    }

    #[test]
    fn plugin_remove_then_add_rejects_the_removed_generation_token() {
        use crate::runtime::lifecycle::ComponentGraphError;

        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let mut graph = components.lifecycle.graph().clone();
        let stale = graph
            .suspend(RUNTIME_WAKE.component_id)
            .expect("runtime wake should produce a stale token")
            .deactivation_requests
            .into_iter()
            .find(|token| token.component_id() == RUNTIME_WAKE.component_id)
            .expect("runtime wake deactivation token should exist");
        let builtin_desired =
            builtin_desired_composition().expect("builtin desired state should validate");
        let desired_without_wake = builtin_desired
            .iter()
            .filter(|(component_id, _)| {
                component_id.as_str() != RUNTIME_WAKE.component_id
                    && component_id.as_str() != UI_RUNTIME_BRIDGE_COMPONENT
            })
            .map(|(component_id, plugin_type)| (component_id.as_str(), plugin_type.clone()))
            .collect::<Vec<_>>();

        components
            .reconcile_plugin_composition(
                DesiredPluginComposition::try_new(desired_without_wake)
                    .expect("removal desired state should validate"),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("remove transaction should commit");
        assert!(
            components
                .lifecycle
                .state(RUNTIME_WAKE.component_id)
                .is_none()
        );

        components
            .reconcile_plugin_composition(
                builtin_desired_composition().expect("builtin desired state should validate"),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("add transaction should commit");
        assert!(
            components
                .lifecycle
                .state(RUNTIME_WAKE.component_id)
                .is_some()
        );
        assert!(matches!(
            components.lifecycle.complete_deactivation(stale),
            Err(ComponentGraphError::StaleDeactivation { component_id, .. })
                if component_id == RUNTIME_WAKE.component_id
        ));
    }

    #[test]
    fn plugin_factory_failure_preserves_live_runtime_authority() {
        const FAILING_PLUGIN: &str = "runtime-event-stream-failing";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_descriptors = components.plugin_descriptor_snapshots();
        let old_desired = components.plugin_loader.desired().clone();
        let old_components = components.lifecycle.components();
        let old_capabilities = components.lifecycle.capabilities();
        let old_context = components.lifecycle.context_snapshots();
        let old_scopes = components.effect_scope_snapshots();
        let constructions = Arc::new(Mutex::new(0_u32));
        let observed_constructions = Arc::clone(&constructions);
        let catalog = PluginFactoryCatalog::try_new([PluginFactory::new(
            builtin_descriptor(FAILING_PLUGIN, "Failing runtime event stream")
                .provides(RUNTIME_EVENT_STREAM.capability)
                .build()
                .expect("failing descriptor should validate"),
            move || {
                *observed_constructions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
                Err("SENSITIVE_FACTORY_FAILURE".to_string().into())
            },
        )])
        .expect("failing catalog should validate");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &catalog,
                desired_with_runtime_event_plugin(FAILING_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("factory failure should abort before lifecycle mutation");

        assert_eq!(*constructions.lock().unwrap(), 1);
        assert!(!error.contains("SENSITIVE_FACTORY_FAILURE"));
        assert_eq!(components.plugin_descriptor_snapshots(), old_descriptors);
        assert_eq!(components.plugin_loader.desired(), &old_desired);
        assert_eq!(components.lifecycle.components(), old_components);
        assert_eq!(components.lifecycle.capabilities(), old_capabilities);
        assert_eq!(components.lifecycle.context_snapshots(), old_context);
        assert_eq!(components.effect_scope_snapshots(), old_scopes);
    }

    #[test]
    fn plugin_graph_preflight_failure_preserves_committed_loader_input() {
        const INVALID_PLUGIN: &str = "runtime-event-stream-invalid";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_descriptors = components.plugin_descriptor_snapshots();
        let old_desired = components.plugin_loader.desired().clone();
        let catalog = PluginFactoryCatalog::try_new([runtime_plugin_factory(
            builtin_descriptor(INVALID_PLUGIN, "Invalid runtime event stream")
                .provides(RUNTIME_EVENT_STREAM.capability)
                .provides(LLM_PORT.capability),
            RuntimeComponents::activate_runtime_event_stream,
            RuntimeComponents::quiesce_noop,
        )])
        .expect("invalid replacement catalog should validate its descriptor");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &catalog,
                desired_with_runtime_event_plugin(INVALID_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("graph preflight should reject duplicate capability provider");

        assert!(error.contains("graph"));
        assert_eq!(components.plugin_descriptor_snapshots(), old_descriptors);
        assert_eq!(components.plugin_loader.desired(), &old_desired);
        components
            .validate_context_alignment()
            .expect("failed desired input must not desynchronize Context");
    }

    #[test]
    fn fresh_plugin_activation_failure_keeps_fresh_authority() {
        const FAILING_PLUGIN: &str = "runtime-event-stream-rejected";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let catalog = PluginFactoryCatalog::try_new([runtime_event_replacement_factory(
            FAILING_PLUGIN,
            reject_runtime_event_activation,
        )])
        .expect("replacement catalog should validate");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &catalog,
                desired_with_runtime_event_plugin(FAILING_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("fresh activation failure should be reported");

        assert!(!format!("{error:?}").contains("SENSITIVE_REPLACEMENT_ACTIVATION"));
        let descriptor = components
            .plugin_descriptor_snapshots()
            .into_iter()
            .find(|snapshot| snapshot.component_id == RUNTIME_EVENT_STREAM.component_id)
            .expect("fresh descriptor should remain authoritative");
        assert_eq!(descriptor.plugin_type, FAILING_PLUGIN);
        assert_eq!(
            components
                .plugin_loader
                .desired()
                .iter()
                .find(|(component_id, _)| {
                    component_id.as_str() == RUNTIME_EVENT_STREAM.component_id
                })
                .expect("fresh desired input should remain committed")
                .1
                .as_str(),
            FAILING_PLUGIN
        );
        assert_eq!(
            components
                .lifecycle
                .state(RUNTIME_EVENT_STREAM.component_id),
            Some(ComponentState::Failed)
        );
        assert!(
            components
                .lifecycle
                .context_snapshots()
                .iter()
                .all(|snapshot| snapshot.key != RUNTIME_EVENT_STREAM.capability)
        );
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

        components
            .validate_context_alignment()
            .expect("shutdown graph and context should both be empty");
        assert!(components.prompt_assembly.manager_snapshot().is_none());
        assert!(components.prompt_assembly.inspection_snapshot().is_empty());
        assert!(components.tool_catalog.definitions().is_empty());
        assert!(components.llm_port.inspection_snapshot().is_empty());
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Disposed)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Disposed)
        );
    }

    #[test]
    fn runtime_event_stream_removal_reactively_quiesces_every_notifier_consumer() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.remove_capability(
                    RUNTIME_EVENT_STREAM.component_id,
                    &CapabilityKey::from(RUNTIME_EVENT_STREAM.capability),
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event stream removal should quiesce its dependent closure");

        for component_id in [
            PERMISSION_POLICY.component_id,
            NATIVE_AGENT_RUNTIME_COMPONENT,
            MODEL_REFRESH_COMPONENT,
            CONTEXT_BUDGET_COMPONENT,
            SESSION_PERSISTENCE.component_id,
            UI_RUNTIME_BRIDGE_COMPONENT,
        ] {
            assert_eq!(
                components.lifecycle.state(component_id),
                Some(ComponentState::Pending),
                "{component_id} must not remain active without runtime_event_stream"
            );
        }
        components
            .validate_context_alignment()
            .expect("dependency reaction must keep graph and context aligned");
        assert!(!components.agent_port().is_busy());
        assert!(!components.session_store_worker.is_running());
    }

    #[test]
    fn runtime_event_stream_republication_reactivates_consumers_on_the_fresh_generation() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let capability = CapabilityKey::from(RUNTIME_EVENT_STREAM.capability);
        let initial_generation = components
            .lifecycle
            .graph()
            .capability(&capability)
            .expect("event stream should initially be visible")
            .generation;

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event stream suspension should quiesce its dependent closure");
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.activate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event stream provider should publish a fresh generation");

        let replacement_generation = components
            .lifecycle
            .graph()
            .capability(&capability)
            .expect("event stream should be republished")
            .generation;
        assert!(replacement_generation > initial_generation);
        let event_stream = components
            .require::<RuntimeEventStreamCapability>()
            .expect("typed Context should expose the replacement event stream");
        assert_eq!(event_stream.generation(), replacement_generation);
        assert_eq!(
            components
                .lifecycle
                .state(RUNTIME_EVENT_STREAM.component_id),
            Some(ComponentState::Active)
        );
        for component_id in [
            PERMISSION_POLICY.component_id,
            NATIVE_AGENT_RUNTIME_COMPONENT,
            MODEL_REFRESH_COMPONENT,
            CONTEXT_BUDGET_COMPONENT,
            SESSION_PERSISTENCE.component_id,
        ] {
            assert_eq!(
                components.lifecycle.state(component_id),
                Some(ComponentState::Active),
                "{component_id} must reactivate against the replacement event stream"
            );
        }
        components
            .validate_context_alignment()
            .expect("republication must keep graph and context aligned");
        assert!(components.permission_policy.is_active_for_test());
        assert!(components.session_store_worker.is_running());
    }

    #[test]
    fn event_stream_republication_preserves_session_backend_and_agent_configuration() {
        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut options = AppRuntimeOptions {
            session_store: Some(store),
            initial_prompt_assembly: Some(manager_with_section("stable", "stable body")),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let backend_generation = components
            .lifecycle
            .graph()
            .capability(&CapabilityKey::from(SESSION_PERSISTENCE.capability))
            .expect("session backend should be visible")
            .generation;
        let original_system_prompt = agent_system_prompt(&components);
        let original_tool_names = components
            .session_workspace_tools
            .definitions()
            .definitions()
            .map(|definition| definition.name.clone())
            .collect::<Vec<_>>();

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event stream suspension should quiesce consumers");
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.activate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event stream activation should restore consumers");

        let backend_lease = components
            .require::<SessionPersistenceCapability>()
            .expect("session backend should remain visible after dependency replacement");
        assert_eq!(backend_lease.generation(), backend_generation + 1);
        assert!(components.session_port.is_some());
        assert!(components.session_backend_views.is_some());
        assert_eq!(agent_system_prompt(&components), original_system_prompt);
        assert_eq!(
            components
                .session_workspace_tools
                .definitions()
                .definitions()
                .map(|definition| definition.name.clone())
                .collect::<Vec<_>>(),
            original_tool_names
        );
        components
            .validate_context_alignment()
            .expect("restored composition should keep graph and Context aligned");
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

        components
            .validate_context_alignment()
            .expect("ephemeral composition should align graph and context");
        assert!(
            components
                .optional::<SessionPersistenceCapability>()
                .expect("typed session lookup should succeed")
                .is_none()
        );
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
        assert!(components.agent_port().is_idle_empty_session());
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

        components
            .validate_context_alignment()
            .expect("reset graph and context should align");
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
        components
            .validate_context_alignment()
            .expect("failed remount should leave graph and context equally absent");
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

        components
            .reset_after_clear(&options)
            .expect("a clean retry should mount a fresh composition");
        components
            .validate_context_alignment()
            .expect("retry graph and context should align");
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state("model_refresh"),
            Some(ComponentState::Active)
        );
        for capability in [
            "llm_port",
            "model_catalog",
            "prompt_assembly",
            "tool_catalog",
        ] {
            assert!(
                components
                    .lifecycle
                    .has_capability(&CapabilityKey::from(capability)),
                "clean retry should publish {capability}"
            );
        }
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
        components
            .validate_context_alignment()
            .expect("permission replacement graph and context should align");
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
        assert!(components.agent_port().is_idle_empty_session());
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
        let lease = components
            .require::<SessionPersistenceCapability>()
            .expect("replacement backend should be visible through context");
        assert_eq!(lease.generation(), after_generation);
        assert_eq!(lease.provider_component(), SESSION_PERSISTENCE.component_id);
        components
            .validate_context_alignment()
            .expect("backend replacement graph and context should align");
        assert!(components.session_backend_views.is_some());
        assert_eq!(
            components.lifecycle.state(NATIVE_AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
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

        assert!(
            components
                .permission_policy
                .inspection_snapshot()
                .iter()
                .any(|provider| provider.provider_id == "replacement-provider")
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

    #[test]
    fn reset_epoch_preflight_preserves_the_live_composition() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let capabilities_before = components.lifecycle.capabilities();
        components.lifecycle.inject_epoch_exhaustion("llm_port");

        let error = components
            .reset_after_clear(&options)
            .expect_err("provider epoch exhaustion should reject reset before cleanup");

        assert!(error.contains("component `llm_port` activation epoch is exhausted"));
        assert_eq!(components.lifecycle.capabilities(), capabilities_before);
        components
            .validate_context_alignment()
            .expect("preflight rejection must preserve graph and context");
        assert_eq!(
            components.lifecycle.state("llm_port"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state("native_agent_runtime"),
            Some(ComponentState::Active)
        );
        assert!(!components.llm_port.inspection_snapshot().is_empty());
    }

    #[test]
    fn observed_consumer_epoch_preflight_preserves_the_session_backend() {
        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut options = AppRuntimeOptions {
            session_store: Some(store.clone()),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let capabilities_before = components.lifecycle.capabilities();
        components
            .lifecycle
            .inject_epoch_exhaustion(NATIVE_AGENT_RUNTIME_COMPONENT);

        let error = components
            .replace_session_backend(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
            )
            .expect_err("observed consumer exhaustion should reject replacement before cleanup");

        assert!(error.contains("component `native_agent_runtime` activation epoch is exhausted"));
        assert_eq!(components.lifecycle.capabilities(), capabilities_before);
        assert!(components.session_port.is_some());
        assert!(components.session_backend_views.is_some());
        assert_eq!(
            components.lifecycle.state(NATIVE_AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert!(Arc::strong_count(&store) > 1);
    }
}
