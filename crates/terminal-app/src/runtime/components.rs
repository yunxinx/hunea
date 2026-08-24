use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use conversation_runtime::ModelRefreshWorker;
use extension_runtime::{ExtensionToolMount, ExtensionToolSet, ExtensionToolSetSource};
use runtime_domain::event_notifier::{RuntimeEventBinding, RuntimeEventNotifier};
use tool_runtime::{ToolCatalog, ToolExecutorRegistry, ToolRegistration};

use super::{
    AppRuntimeOptions,
    agent::{
        AgentRuntimeFactory, AgentRuntimeMount, AgentRuntimePort, AgentSessionCapability,
        construct_native_agent_runtime,
    },
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
    workspace_tools::conversation_workspace_tool_catalog,
};
use runtime_domain::runtime_wake::RuntimeWake;

#[cfg(test)]
use super::agent::{ReplayFixture, ReplayLifecycleProbe};

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

const AGENT_RUNTIME_COMPONENT: &str = "agent_runtime";
const MODEL_REFRESH_COMPONENT: &str = "model_refresh";
const CONTEXT_BUDGET_COMPONENT: &str = "context_budget";
const UI_RUNTIME_BRIDGE_COMPONENT: &str = "ui_runtime_bridge";
const EXTERNAL_EXTENSION_COMPONENT: &str = "external_extension_tools";

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
const EXTERNAL_EXTENSION_PLUGIN: &str = "stdio-extension-tools";

type RuntimePluginActivation = for<'a> fn(
    &mut RuntimeComponents,
    &EffectScope,
    &mut ComponentActivationContext<'a>,
    ComponentLifecycleMode,
) -> Result<ComponentActivationOutcome, String>;
type RuntimePluginQuiescence =
    fn(&mut RuntimeComponents, ComponentLifecycleMode) -> Result<(), String>;

#[derive(Clone, Copy)]
struct RuntimePluginLifecycle {
    activate: RuntimePluginActivation,
    quiesce: RuntimePluginQuiescence,
}

#[derive(Clone)]
enum RuntimePluginImplementationKind {
    Component,
    AgentRuntime(AgentRuntimeFactory),
}

#[derive(Clone)]
struct RuntimePluginImplementation {
    lifecycle: RuntimePluginLifecycle,
    kind: RuntimePluginImplementationKind,
}

impl RuntimePluginImplementation {
    fn component(activate: RuntimePluginActivation, quiesce: RuntimePluginQuiescence) -> Self {
        Self {
            lifecycle: RuntimePluginLifecycle { activate, quiesce },
            kind: RuntimePluginImplementationKind::Component,
        }
    }

    fn agent_runtime(factory: AgentRuntimeFactory) -> Self {
        Self {
            lifecycle: RuntimePluginLifecycle {
                activate: RuntimeComponents::activate_agent_runtime,
                quiesce: RuntimeComponents::quiesce_agent_runtime,
            },
            kind: RuntimePluginImplementationKind::AgentRuntime(factory),
        }
    }

    fn activate(
        &self,
        components: &mut RuntimeComponents,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        (self.lifecycle.activate)(components, scope, context, mode)
    }

    fn quiesce(
        &self,
        components: &mut RuntimeComponents,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        (self.lifecycle.quiesce)(components, mode)
    }

    fn construct_agent_runtime(
        &self,
        mount: AgentRuntimeMount,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        match &self.kind {
            RuntimePluginImplementationKind::AgentRuntime(factory) => factory.construct(mount),
            RuntimePluginImplementationKind::Component => {
                Err("plugin implementation does not provide an Agent runtime factory".to_string())
            }
        }
    }
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

fn agent_runtime_descriptor(
    plugin_type: &'static str,
    display_name: &'static str,
) -> PluginDescriptorBuilder {
    builtin_descriptor(plugin_type, display_name)
        .requires(RUNTIME_EVENT_STREAM.capability)
        .requires(LLM_PORT.capability)
        .requires(MODEL_CATALOG.capability)
        .requires(PERMISSION_POLICY.capability)
        .requires(PROMPT_ASSEMBLY.capability)
        .requires(TOOL_CATALOG.capability)
        .observes(SESSION_PERSISTENCE.capability)
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
        desired_builtin(AGENT_RUNTIME_COMPONENT, NATIVE_AGENT_RUNTIME_PLUGIN),
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
    let implementation = RuntimePluginImplementation::component(activate, quiesce);
    PluginFactory::new(
        descriptor
            .build()
            .expect("builtin plugin descriptor must be valid"),
        move || Ok(implementation.clone()),
    )
}

fn agent_runtime_plugin_factory(
    descriptor: PluginDescriptorBuilder,
    factory: AgentRuntimeFactory,
) -> PluginFactory<RuntimePluginImplementation> {
    let implementation = RuntimePluginImplementation::agent_runtime(factory);
    PluginFactory::new(
        descriptor
            .build()
            .expect("builtin plugin descriptor must be valid"),
        move || Ok(implementation.clone()),
    )
}

#[cfg(test)]
fn replay_agent_replacement_factory(
    plugin_type: &'static str,
    fixture: ReplayFixture,
    lifecycle_probe: Option<Arc<ReplayLifecycleProbe>>,
) -> PluginFactory<RuntimePluginImplementation> {
    agent_runtime_plugin_factory(
        builtin_descriptor(plugin_type, "Replay Agent loop")
            .requires(RUNTIME_EVENT_STREAM.capability),
        AgentRuntimeFactory::replay(fixture, lifecycle_probe),
    )
}

#[cfg(test)]
fn desired_with_agent_plugin_for_test(
    plugin_type: Option<&'static str>,
) -> DesiredPluginComposition {
    let desired = builtin_desired_composition().expect("builtin desired state should validate");
    DesiredPluginComposition::try_new(desired.iter().filter_map(|(component_id, current_type)| {
        if component_id.as_str() != AGENT_RUNTIME_COMPONENT {
            return Some((component_id.as_str(), current_type.clone()));
        }
        plugin_type.map(|plugin_type| (component_id.as_str(), builtin_plugin_type(plugin_type)))
    }))
    .expect("Agent replacement desired state should validate")
}

fn builtin_plugin_catalog()
-> Result<PluginFactoryCatalog<RuntimePluginImplementation>, PluginCatalogError> {
    builtin_plugin_catalog_with_agent_factory(AgentRuntimeFactory::new(
        construct_native_agent_runtime,
    ))
}

fn builtin_plugin_catalog_with_agent_factory(
    agent_runtime_factory: AgentRuntimeFactory,
) -> Result<PluginFactoryCatalog<RuntimePluginImplementation>, PluginCatalogError> {
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
        agent_runtime_plugin_factory(
            agent_runtime_descriptor(NATIVE_AGENT_RUNTIME_PLUGIN, "Native agent loop"),
            agent_runtime_factory,
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
        runtime_plugin_factory(
            builtin_descriptor(EXTERNAL_EXTENSION_PLUGIN, "External extension tools")
                .requires(TOOL_CATALOG.capability),
            RuntimeComponents::activate_external_extension,
            RuntimeComponents::quiesce_external_extension,
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
    extension_tool_set: Option<ExtensionToolSet>,
    extension_source: Option<Arc<dyn ExtensionToolSetSource>>,
    is_session_backend_replacement: bool,
}

struct PreparedPluginCommit {
    desired: DesiredPluginComposition,
    reconciliation: PreparedPluginReconciliation<RuntimePluginImplementation>,
    agent_runtime: PreparedAgentRuntimeCommit,
}

enum PreparedAgentRuntimeCommit {
    Keep,
    Replace(Box<PreparedAgentRuntimeReplacement>),
}

struct PreparedAgentRuntimeReplacement {
    mount: Option<AgentRuntimeMount>,
    candidate: Option<Box<dyn AgentRuntimePort>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentPluginReconciliation {
    Keep,
    Replace,
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
    extension_mount: Option<Arc<Mutex<ExtensionToolMount>>>,
    extension_source: Option<Arc<dyn ExtensionToolSetSource>>,
    runtime_event_notifier: RuntimeEventNotifier,
    activation_staging: ComponentActivationStaging,
    plugin_loader: PluginCompositionLoader<RuntimePluginImplementation>,
    plugins: PluginComposition<RuntimePluginImplementation>,
    prepared_plugin_commit: Option<PreparedPluginCommit>,
    is_agent_replacement_activating: bool,
    #[cfg(test)]
    plugin_transaction_trace: Option<Arc<Mutex<Vec<String>>>>,
    pub(super) lifecycle: ComponentLifecycleExecutor,
    is_shutdown: bool,
    shutdown_lifecycle_error: Option<String>,
}

fn construct_agent_runtime(
    implementation: &RuntimePluginImplementation,
    mount: AgentRuntimeMount,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    implementation.construct_agent_runtime(mount)
}

fn construct_committed_agent_runtime(
    plugins: &PluginComposition<RuntimePluginImplementation>,
    mount: AgentRuntimeMount,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    let implementation = plugins
        .implementation(AGENT_RUNTIME_COMPONENT)
        .ok_or_else(|| "Agent component has no committed plugin implementation".to_string())?;
    construct_agent_runtime(implementation, mount)
}

fn classify_agent_plugin_reconciliation(
    observed: Option<&PluginTypeId>,
    desired: Option<&PluginTypeId>,
) -> Result<AgentPluginReconciliation, String> {
    match (observed, desired) {
        (Some(observed), Some(desired)) if observed == desired => {
            Ok(AgentPluginReconciliation::Keep)
        }
        (Some(_), Some(_)) => Ok(AgentPluginReconciliation::Replace),
        _ => Err("Runtime composition must contain exactly one Agent plugin".to_string()),
    }
}

impl RuntimeComponents {
    pub(super) fn agent_port(&self) -> &dyn AgentRuntimePort {
        &*self.agent_runtime
    }

    pub(super) fn agent_port_mut(&mut self) -> &mut dyn AgentRuntimePort {
        &mut *self.agent_runtime
    }

    pub(super) fn agent_session(&self) -> Result<&dyn AgentSessionCapability, String> {
        self.agent_runtime
            .session()
            .ok_or_else(|| "Agent adapter does not provide session capability".to_string())
    }

    pub(super) fn agent_session_mut(&mut self) -> Result<&mut dyn AgentSessionCapability, String> {
        self.agent_runtime
            .session_mut()
            .ok_or_else(|| "Agent adapter does not provide session capability".to_string())
    }

    #[cfg(test)]
    pub(super) fn replace_agent_with_replay_for_test(
        &mut self,
        options: &AppRuntimeOptions,
        fixture: ReplayFixture,
    ) -> Result<(), String> {
        const REPLAY_AGENT: &str = "replay-agent-loop";
        let catalog = PluginFactoryCatalog::try_new([replay_agent_replacement_factory(
            REPLAY_AGENT,
            fixture,
            None,
        )])
        .map_err(|error| error.to_string())?;
        self.reconcile_plugin_composition_with_catalog(
            options,
            &catalog,
            desired_with_agent_plugin_for_test(Some(REPLAY_AGENT)),
            ComponentLifecycleMode::Reconfigure,
        )
    }

    #[cfg(test)]
    pub(super) fn agent_test_harness(&mut self) -> &mut dyn super::agent::AgentRuntimeTestHarness {
        self.agent_runtime
            .session_mut()
            .expect("runtime test requires an Agent session capability")
            .test_harness()
            .expect("runtime test requires an Agent fixture harness")
    }

    #[cfg(test)]
    pub(super) fn agent_test_harness_ref(&self) -> &dyn super::agent::AgentRuntimeTestHarness {
        self.agent_runtime
            .session()
            .expect("runtime test requires an Agent session capability")
            .test_harness_ref()
            .expect("runtime test requires an Agent fixture harness")
    }

    pub(super) fn new(options: &mut AppRuntimeOptions) -> Result<Self, String> {
        let desired_plugins = builtin_desired_composition().map_err(|error| error.to_string())?;
        let plugin_catalog = builtin_plugin_catalog().map_err(|error| error.to_string())?;
        Self::new_with_plugin_catalog(options, desired_plugins, plugin_catalog)
    }

    #[cfg(test)]
    fn new_with_agent_runtime_factory(
        options: &mut AppRuntimeOptions,
        factory: AgentRuntimeFactory,
    ) -> Result<Self, String> {
        let desired_plugins = builtin_desired_composition().map_err(|error| error.to_string())?;
        let plugin_catalog = builtin_plugin_catalog_with_agent_factory(factory)
            .map_err(|error| error.to_string())?;
        Self::new_with_plugin_catalog(options, desired_plugins, plugin_catalog)
    }

    fn new_with_plugin_catalog(
        options: &mut AppRuntimeOptions,
        desired_plugins: DesiredPluginComposition,
        plugin_catalog: PluginFactoryCatalog<RuntimePluginImplementation>,
    ) -> Result<Self, String> {
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
        let agent_runtime = construct_committed_agent_runtime(
            &plugins,
            AgentRuntimeMount::new(
                options,
                session_workspace_tools.clone(),
                prompt_assembly_tool_definitions,
                prompt_assembly_snapshot,
                session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                llm_port.clone(),
                permission_policy.clone(),
                TERMINAL_APPROVAL_PROVIDER_ID.to_string(),
            ),
        )?;
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
            extension_mount: None,
            extension_source: None,
            runtime_event_notifier,
            activation_staging: ComponentActivationStaging {
                approval_registration: Some(approval_registration),
                provider_registrations: Some(provider_registrations),
                tool_registration: Some(tool_registration),
                prompt_registration: Some(prompt_registration),
                session_backend_registration,
                runtime_wake: None,
                extension_tool_set: None,
                extension_source: None,
                is_session_backend_replacement: false,
            },
            plugin_loader,
            plugins,
            prepared_plugin_commit: None,
            is_agent_replacement_activating: false,
            #[cfg(test)]
            plugin_transaction_trace: None,
            lifecycle: ComponentLifecycleExecutor::default(),
            is_shutdown: false,
            shutdown_lifecycle_error: None,
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

    /// 将已完成 handshake 的 extension tool set 接入 lifecycle。
    ///
    /// discovery/stdio 启动在 async 边界外完成；此处只接收 opaque set，并让 graph 决定
    /// `tool_catalog` 缺失时的 Pending 状态。默认 composition 不包含该 component。
    pub(super) fn mount_extension_tool_set(
        &mut self,
        tool_set: ExtensionToolSet,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let source = tool_set
            .rediscovery_source()
            .ok_or_else(|| "external extension discovery source is not available".to_string())?;
        if self
            .plugins
            .implementation(EXTERNAL_EXTENSION_COMPONENT)
            .is_some()
        {
            return self.replace_extension_tool_set(tool_set);
        }
        self.activation_staging.extension_tool_set = Some(tool_set);
        self.activation_staging.extension_source = Some(source);
        let desired = match self.desired_with_external_extension(true) {
            Ok(desired) => desired,
            Err(error) => {
                self.activation_staging.extension_tool_set.take();
                self.activation_staging.extension_source.take();
                return Err(error);
            }
        };
        let reconciliation = match self
            .plugin_loader
            .prepare_reconciliation(&desired, &self.plugins)
        {
            Ok(reconciliation) => reconciliation,
            Err(error) => {
                self.activation_staging.extension_tool_set.take();
                self.activation_staging.extension_source.take();
                return Err(error.to_string());
            }
        };
        let result = self.commit_prepared_plugin_reconciliation(
            desired,
            reconciliation,
            PreparedAgentRuntimeCommit::Keep,
            ComponentLifecycleMode::Reconfigure,
        );
        if result.is_err() {
            self.activation_staging.extension_tool_set.take();
            self.activation_staging.extension_source.take();
        }
        result
    }

    /// 先撤销旧 extension component，再接入 fresh set；两个 generation 不会并存。
    pub(super) fn replace_extension_tool_set(
        &mut self,
        tool_set: ExtensionToolSet,
    ) -> Result<(), String> {
        self.remove_extension_tool_set()?;
        self.mount_extension_tool_set(tool_set)
    }

    /// 从 desired composition 移除 extension component，并执行其 effect inverse。
    pub(super) fn remove_extension_tool_set(&mut self) -> Result<(), String> {
        self.activation_staging.extension_tool_set.take();
        self.activation_staging.extension_source.take();
        if self
            .plugins
            .implementation(EXTERNAL_EXTENSION_COMPONENT)
            .is_none()
        {
            self.extension_source = None;
            return Ok(());
        }
        let desired = self.desired_with_external_extension(false)?;
        let reconciliation = self
            .plugin_loader
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        let result = self.commit_prepared_plugin_reconciliation(
            desired,
            reconciliation,
            PreparedAgentRuntimeCommit::Keep,
            ComponentLifecycleMode::Reconfigure,
        );
        if result.is_ok() {
            self.extension_source = None;
        }
        result
    }

    fn desired_with_external_extension(
        &self,
        include_external_extension: bool,
    ) -> Result<DesiredPluginComposition, String> {
        let mut entries = self
            .plugins
            .observed()
            .iter()
            .filter(|(component_id, _)| component_id.as_str() != EXTERNAL_EXTENSION_COMPONENT)
            .map(|(component_id, plugin_type)| {
                (component_id.as_str().to_string(), plugin_type.clone())
            })
            .collect::<Vec<_>>();
        if include_external_extension {
            entries.push((
                EXTERNAL_EXTENSION_COMPONENT.to_string(),
                PluginTypeId::try_new(EXTERNAL_EXTENSION_PLUGIN)
                    .map_err(|error| error.to_string())?,
            ));
        }
        DesiredPluginComposition::try_new(entries).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn external_extension_state(&self) -> Option<super::lifecycle::ComponentState> {
        self.lifecycle.state(EXTERNAL_EXTENSION_COMPONENT)
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
        options: &AppRuntimeOptions,
        desired: DesiredPluginComposition,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        if self.is_shutdown {
            return Err("Runtime components are shut down".to_string());
        }
        let agent_runtime = self.prepare_agent_runtime_commit(options, &desired)?;
        let reconciliation = self
            .plugin_loader
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        self.commit_prepared_plugin_reconciliation(desired, reconciliation, agent_runtime, mode)
    }

    #[cfg(test)]
    fn reconcile_plugin_composition_with_catalog(
        &mut self,
        options: &AppRuntimeOptions,
        catalog: &PluginFactoryCatalog<RuntimePluginImplementation>,
        desired: DesiredPluginComposition,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let agent_runtime = self.prepare_agent_runtime_commit(options, &desired)?;
        let reconciliation = catalog
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        self.commit_prepared_plugin_reconciliation(desired, reconciliation, agent_runtime, mode)
    }

    fn prepare_agent_runtime_commit(
        &self,
        options: &AppRuntimeOptions,
        desired: &DesiredPluginComposition,
    ) -> Result<PreparedAgentRuntimeCommit, String> {
        let desired_type = desired
            .iter()
            .find(|(component_id, _)| component_id.as_str() == AGENT_RUNTIME_COMPONENT)
            .map(|(_, plugin_type)| plugin_type);
        let observed = self.plugins.observed();
        let observed_type = observed
            .iter()
            .find(|(component_id, _)| component_id.as_str() == AGENT_RUNTIME_COMPONENT)
            .map(|(_, plugin_type)| plugin_type);
        match classify_agent_plugin_reconciliation(observed_type, desired_type)? {
            AgentPluginReconciliation::Keep => Ok(PreparedAgentRuntimeCommit::Keep),
            AgentPluginReconciliation::Replace => Ok(PreparedAgentRuntimeCommit::Replace(
                Box::new(PreparedAgentRuntimeReplacement {
                    mount: Some(self.agent_runtime_mount(options, self.current_session_port())),
                    candidate: None,
                }),
            )),
        }
    }

    fn commit_prepared_plugin_reconciliation(
        &mut self,
        desired: DesiredPluginComposition,
        reconciliation: PreparedPluginReconciliation<RuntimePluginImplementation>,
        agent_runtime: PreparedAgentRuntimeCommit,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let definitions = reconciliation.definitions();
        debug_assert!(self.prepared_plugin_commit.is_none());
        self.prepared_plugin_commit = Some(PreparedPluginCommit {
            desired,
            reconciliation,
            agent_runtime,
        });
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.reconcile_definitions_with_commit(definitions, components, mode)
        })
        .map_err(|error| format!("{error:?}"))
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
        // 旧 adapter 完全 quiescent 后才请 committed plugin 构造新 generation；失败时
        // capability 保持 removed，所有尚未发布的 fresh registrations 由 Drop 逆向撤销。
        let fresh_agent_runtime = construct_committed_agent_runtime(
            &self.plugins,
            AgentRuntimeMount::new(
                options,
                session_workspace_tools.clone(),
                prompt_assembly_tool_definitions,
                fresh_prompt_assembly_snapshot,
                self.session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                fresh_llm_port.clone(),
                self.permission_policy.clone(),
                self.permission_provider_id.clone(),
            ),
        )?;
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

    /// 在全部 consumer 与旧 worker quiesce 后替换当前 session backend。
    #[allow(dead_code)]
    pub(super) fn replace_session_backend(
        &mut self,
        options: &AppRuntimeOptions,
        store: Arc<dyn session_store::SessionStore>,
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
                [SESSION_PERSISTENCE.component_id, AGENT_RUNTIME_COMPONENT],
            )
            .map_err(|error| error.to_string())?;
        self.activation_staging.is_session_backend_replacement = true;
        if let Err(error) = self.with_lifecycle(|lifecycle, components| {
            lifecycle.deactivate_components(
                [AGENT_RUNTIME_COMPONENT, SESSION_PERSISTENCE.component_id],
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
        let fresh_agent_runtime =
            match self.fresh_agent_runtime(options, Some(Arc::clone(&fresh_views.port))) {
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
                [SESSION_PERSISTENCE.component_id, AGENT_RUNTIME_COMPONENT],
                components,
                ComponentLifecycleMode::Reconfigure,
            )
        })
        .map_err(|error| error.to_string())
    }

    fn fresh_agent_runtime(
        &self,
        options: &AppRuntimeOptions,
        session_port: Option<Arc<dyn session_store::SessionPort>>,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        construct_committed_agent_runtime(
            &self.plugins,
            self.agent_runtime_mount(options, session_port),
        )
    }

    fn current_session_port(&self) -> Option<Arc<dyn session_store::SessionPort>> {
        self.session_backend_views
            .as_ref()
            .map(|views| Arc::clone(&views.port))
    }

    fn agent_runtime_mount(
        &self,
        options: &AppRuntimeOptions,
        session_port: Option<Arc<dyn session_store::SessionPort>>,
    ) -> AgentRuntimeMount {
        AgentRuntimeMount::new(
            options,
            self.session_workspace_tools.clone(),
            self.tool_catalog.definitions(),
            self.prompt_assembly.session_snapshot(),
            session_port,
            self.llm_port.clone(),
            self.permission_policy.clone(),
            self.permission_provider_id.clone(),
        )
    }

    fn restore_ephemeral_session_consumers(
        &mut self,
        options: &AppRuntimeOptions,
    ) -> Result<(), String> {
        let fresh_agent_runtime = self
            .fresh_agent_runtime(options, None)
            .map_err(|error| format!("restore ephemeral Agent after backend failure: {error}"))?;
        self.agent_runtime = fresh_agent_runtime;
        self.session_store_worker = SessionStoreWorker::default();
        self.with_lifecycle(|lifecycle, components| {
            lifecycle.activate_components(
                [AGENT_RUNTIME_COMPONENT],
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
        let fresh_agent_runtime = match construct_committed_agent_runtime(
            &self.plugins,
            AgentRuntimeMount::new(
                options,
                self.session_workspace_tools.clone(),
                self.tool_catalog.definitions(),
                self.prompt_assembly.session_snapshot(),
                self.session_backend_views
                    .as_ref()
                    .map(|views| Arc::clone(&views.port)),
                self.llm_port.clone(),
                fresh_policy.clone(),
                provider_id.clone(),
            ),
        ) {
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

    fn activate_external_extension(
        &mut self,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let catalog = context
            .require::<ToolCatalogCapability>()
            .map_err(|error| error.to_string())?;
        let tool_set = match self.activation_staging.extension_tool_set.take() {
            Some(tool_set) => tool_set,
            None => {
                let source = self.extension_source.clone().ok_or_else(|| {
                    "external extension activation source is not available".to_string()
                })?;
                source
                    .discover()
                    .map_err(|_| "external extension rediscovery failed".to_string())?
            }
        };
        let source = self
            .activation_staging
            .extension_source
            .take()
            .or_else(|| tool_set.rediscovery_source())
            .ok_or_else(|| "external extension discovery source is not available".to_string())?;
        let mount = tool_set
            .mount(&catalog, EXTERNAL_EXTENSION_COMPONENT)
            .map_err(|error| error.to_string())?;
        let mount = Arc::new(Mutex::new(mount));
        self.extension_source = Some(Arc::clone(&source));
        let disposer = Arc::clone(&mount);
        scope
            .register("extension_tool_mount", move || {
                disposer
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .dispose()
                    .map_err(|_| "external extension transport failed to shut down".to_string())
            })
            .map_err(|error| error.to_string())?;
        self.extension_mount = Some(mount);
        Ok(ComponentActivationOutcome::Ready)
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

    fn activate_agent_runtime(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let event_stream = context
            .require::<RuntimeEventStreamCapability>()
            .map_err(|error| error.to_string())?;
        self.agent_runtime.activate(event_stream)?;
        self.is_agent_replacement_activating = false;
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

    fn quiesce_agent_runtime(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        match mode {
            ComponentLifecycleMode::Shutdown => self
                .agent_runtime
                .shutdown()
                .map_err(|_| "Agent adapter failed to shut down".to_string()),
            _ => self
                .agent_runtime
                .suspend()
                .map_err(|_| "Agent adapter failed to suspend".to_string()),
        }
    }

    fn finalize_agent_runtime_replacement(&mut self) -> Result<(), String> {
        self.agent_runtime
            .shutdown()
            .map_err(|_| "Agent adapter failed to finalize for replacement".to_string())
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

    fn quiesce_external_extension(&mut self, _mode: ComponentLifecycleMode) -> Result<(), String> {
        let Some(mount) = self.extension_mount.take() else {
            return Ok(());
        };
        mount
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .dispose()
            .map_err(|_| "external extension transport failed to shut down".to_string())
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
        if !self.is_shutdown {
            self.is_shutdown = true;
            self.shutdown_lifecycle_error = self
                .with_lifecycle(|lifecycle, components| lifecycle.shutdown(components))
                .err()
                .map(|error| format!("{error:?}"));
        }
        let agent_error = self
            .agent_runtime
            .shutdown()
            .err()
            .map(|_| "Agent runtime finalization failed".to_string());
        self.is_agent_replacement_activating = false;
        match (self.shutdown_lifecycle_error.clone(), agent_error) {
            (None, None) => Ok(()),
            (Some(error), None) | (None, Some(error)) => Err(error),
            (Some(lifecycle), Some(agent)) => Err(format!("{lifecycle}; {agent}")),
        }
    }
}

impl ComponentLifecycleCallbacks for RuntimeComponents {
    fn prepare_authority(&mut self) -> Result<(), String> {
        let is_agent_replacement = self
            .prepared_plugin_commit
            .as_ref()
            .is_some_and(|prepared| {
                matches!(
                    &prepared.agent_runtime,
                    PreparedAgentRuntimeCommit::Replace(_)
                )
            });
        if is_agent_replacement {
            self.finalize_agent_runtime_replacement()?;
        }
        {
            let prepared = self
                .prepared_plugin_commit
                .as_mut()
                .ok_or_else(|| "plugin authority preparation is missing".to_string())?;
            match &mut prepared.agent_runtime {
                PreparedAgentRuntimeCommit::Keep => {}
                PreparedAgentRuntimeCommit::Replace(replacement) => {
                    if replacement.candidate.is_some() {
                        return Err("Agent adapter candidate is already prepared".to_string());
                    }
                    let mount = replacement
                        .mount
                        .take()
                        .ok_or_else(|| "Agent adapter mount is no longer available".to_string())?;
                    let implementation = prepared
                        .reconciliation
                        .prospective_implementation(&self.plugins, AGENT_RUNTIME_COMPONENT)
                        .ok_or_else(|| {
                            "Agent replacement has no prospective plugin implementation".to_string()
                        })?;
                    replacement.candidate = Some(construct_agent_runtime(implementation, mount)?);
                }
            }
        }
        #[cfg(test)]
        self.record_plugin_transaction_event("prepare:authority");
        Ok(())
    }

    fn commit_authority(&mut self) {
        let mut prepared = self
            .prepared_plugin_commit
            .take()
            .expect("plugin authority commit must have a prepared composition");
        let candidate = match &mut prepared.agent_runtime {
            PreparedAgentRuntimeCommit::Keep => None,
            PreparedAgentRuntimeCommit::Replace(replacement) => Some(
                replacement
                    .candidate
                    .take()
                    .expect("Agent replacement must prepare its adapter before authority commit"),
            ),
        };
        self.plugins.commit_reconciliation(prepared.reconciliation);
        self.plugin_loader.commit_desired(prepared.desired);
        if let Some(candidate) = candidate {
            self.agent_runtime = candidate;
            self.is_agent_replacement_activating = true;
        }
        #[cfg(test)]
        self.record_plugin_transaction_event("authority:commit");
    }

    fn abort_authority(&mut self) {
        self.prepared_plugin_commit.take();
        #[cfg(test)]
        self.record_plugin_transaction_event("authority:abort");
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
            .cloned()
            .ok_or_else(|| format!("component `{component_id}` has no prepared plugin instance"))?;
        #[cfg(test)]
        self.record_plugin_transaction_event(format!("activate:{component_id}"));
        implementation.activate(self, scope, context, mode)
    }

    fn quiesce_component(
        &mut self,
        component_id: &str,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let implementation = self
            .plugins
            .implementation(component_id)
            .cloned()
            .ok_or_else(|| format!("component `{component_id}` has no prepared plugin instance"))?;
        #[cfg(test)]
        self.record_plugin_transaction_event(format!("quiesce:{component_id}"));
        let plugin_result = implementation.quiesce(self, mode);
        let finalization_result =
            if component_id == AGENT_RUNTIME_COMPONENT && self.is_agent_replacement_activating {
                self.finalize_agent_runtime_replacement()
            } else {
                Ok(())
            };
        if finalization_result.is_ok()
            && component_id == AGENT_RUNTIME_COMPONENT
            && self.is_agent_replacement_activating
        {
            self.is_agent_replacement_activating = false;
        }
        match (plugin_result, finalization_result) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(plugin), Err(finalization)) => Err(format!("{plugin}; {finalization}")),
        }
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
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use super::*;
    use crate::runtime::agent::{AgentRuntimeActivity, AgentSessionRestore};
    use crate::runtime::lifecycle::{ComponentDefinition, ComponentState};
    use extension_protocol::{
        ExtensionCapability, ExtensionMethod, ExtensionRequest, ExtensionResponse,
        InitializeResult, ToolDescriptor, ToolsListResult,
    };
    use extension_runtime::{
        ExtensionDiscoveryError, ExtensionRequestFuture, ExtensionRequestTransport,
        ExtensionToolClient, ExtensionToolOptions, ExtensionToolSetSource, ExtensionTransportError,
    };
    use runtime_domain::agent::{
        AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentRuntime,
        AgentRuntimeError, AgentTurnId, AgentTurnRequest,
    };
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

    #[derive(Clone, Default)]
    struct StaticExtensionTransport {
        shutdowns: Arc<AtomicUsize>,
    }

    impl ExtensionRequestTransport for StaticExtensionTransport {
        fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_> {
            let response = match request.method() {
                ExtensionMethod::Initialize => ExtensionResponse::success(
                    request.request_id().to_string(),
                    InitializeResult {
                        protocol: extension_protocol::PROTOCOL_NAME.to_string(),
                        version: extension_protocol::PROTOCOL_VERSION,
                        capabilities: vec![
                            ExtensionCapability::Cancel,
                            ExtensionCapability::StructuredErrors,
                        ],
                    },
                ),
                ExtensionMethod::ToolsList => ExtensionResponse::success(
                    request.request_id().to_string(),
                    ToolsListResult {
                        tools: vec![ToolDescriptor {
                            name: "extension_echo".to_string(),
                            description: None,
                            input_schema: None,
                        }],
                    },
                ),
                _ => unreachable!("component mount only discovers extension tools"),
            }
            .expect("static extension response should encode");
            Box::pin(async move { Ok(response) })
        }

        fn shutdown(&self) -> Result<(), ExtensionTransportError> {
            self.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct StaticExtensionSource {
        shutdowns: Arc<AtomicUsize>,
        discoveries: Arc<AtomicUsize>,
    }

    impl ExtensionToolSetSource for StaticExtensionSource {
        fn discover(&self) -> Result<extension_runtime::ExtensionToolSet, ExtensionDiscoveryError> {
            self.discoveries.fetch_add(1, Ordering::SeqCst);
            let transport = StaticExtensionTransport {
                shutdowns: Arc::clone(&self.shutdowns),
            };
            let set = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| {
                    ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable)
                })?
                .block_on(
                    ExtensionToolClient::new(transport, ExtensionToolOptions::default()).discover(),
                )?;
            Ok(set.with_rediscovery_source(Arc::new(self.clone())))
        }
    }

    fn discovered_extension_set(
        transport: StaticExtensionTransport,
    ) -> extension_runtime::ExtensionToolSet {
        let source = Arc::new(StaticExtensionSource {
            shutdowns: Arc::clone(&transport.shutdowns),
            discoveries: Arc::new(AtomicUsize::new(0)),
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(
                ExtensionToolClient::new(transport, ExtensionToolOptions::default()).discover(),
            )
            .expect("static extension discovery should succeed")
            .with_rediscovery_source(source)
    }

    fn agent_system_prompt(components: &RuntimeComponents) -> Option<String> {
        components
            .agent_session()
            .expect("Native Agent should provide session capability")
            .context_budget_snapshot()
            .items
            .iter()
            .find(|item| item.role() == Some(provider_protocol::Role::System))
            .map(provider_protocol::ConversationItem::text_content)
    }

    struct RecordingAgentRuntime {
        lifecycle_trace: Arc<Mutex<Vec<&'static str>>>,
        activation_failure: bool,
        shutdown_failures_remaining: usize,
        shutdown_calls: Option<Arc<AtomicUsize>>,
        drop_count: Option<Arc<AtomicUsize>>,
        owned_mount: Option<AgentRuntimeMount>,
        is_shutdown: bool,
        is_finalized: bool,
    }

    impl RecordingAgentRuntime {
        fn new(lifecycle_trace: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: false,
                shutdown_failures_remaining: 0,
                shutdown_calls: None,
                drop_count: None,
                owned_mount: None,
                is_shutdown: true,
                is_finalized: false,
            }
        }

        fn with_shutdown_failure(lifecycle_trace: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: false,
                shutdown_failures_remaining: usize::MAX,
                shutdown_calls: None,
                drop_count: None,
                owned_mount: None,
                is_shutdown: true,
                is_finalized: false,
            }
        }

        fn with_shutdown_counter(
            lifecycle_trace: Arc<Mutex<Vec<&'static str>>>,
            shutdown_calls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: true,
                shutdown_failures_remaining: 0,
                shutdown_calls: Some(shutdown_calls),
                drop_count: None,
                owned_mount: None,
                is_shutdown: true,
                is_finalized: false,
            }
        }

        fn with_abort_probes(
            lifecycle_trace: Arc<Mutex<Vec<&'static str>>>,
            mount: AgentRuntimeMount,
            drop_count: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: false,
                shutdown_failures_remaining: 0,
                shutdown_calls: None,
                drop_count: Some(drop_count),
                owned_mount: Some(mount),
                is_shutdown: true,
                is_finalized: false,
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
            if let Some(shutdown_calls) = &self.shutdown_calls {
                shutdown_calls.fetch_add(1, Ordering::SeqCst);
            }
            if !self.is_finalized {
                self.record("shutdown");
                self.is_shutdown = true;
                self.is_finalized = true;
            }
            if self.shutdown_failures_remaining > 0 {
                self.shutdown_failures_remaining -= 1;
                self.is_finalized = false;
                Err(AgentRuntimeError::Shutdown(
                    "SENSITIVE_AGENT_CLEANUP_FAILURE".to_string(),
                ))
            } else {
                Ok(())
            }
        }
    }

    impl AgentRuntimePort for RecordingAgentRuntime {
        fn activate(
            &mut self,
            _event_stream: CapabilityLease<RuntimeEventStreamCapability>,
        ) -> Result<(), String> {
            if self.activation_failure {
                return Err("SENSITIVE_AGENT_ACTIVATION_FAILURE".to_string());
            }
            self.record("activate");
            self.is_shutdown = false;
            self.is_finalized = false;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            if !self.is_shutdown {
                self.record("suspend");
                self.is_shutdown = true;
            }
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            None
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            None
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    impl Drop for RecordingAgentRuntime {
        fn drop(&mut self) {
            let _ = self.owned_mount.take();
            if let Some(drop_count) = &self.drop_count {
                drop_count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct DropCounter {
        count: Arc<AtomicUsize>,
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.count.fetch_add(1, Ordering::SeqCst);
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
                    [AGENT_RUNTIME_COMPONENT],
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
                    [AGENT_RUNTIME_COMPONENT],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("recording Agent should activate through the lifecycle port");
        let unavailable = match components.agent_session_mut() {
            Ok(_) => panic!("recording Agent must not fabricate a session capability"),
            Err(error) => error,
        };
        assert_eq!(
            unavailable,
            "Agent adapter does not provide session capability"
        );
        assert_eq!(
            components.agent_port().activity(),
            AgentRuntimeActivity::Idle
        );
        assert!(!components.agent_port().has_pending_work());
        assert!(components.agent_port_mut().drain_events().is_empty());
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
        let production_source = source
            .split_once("#[cfg(test)]\nmod tests")
            .map(|(production, _)| production)
            .expect("components source should keep tests behind cfg(test)");
        let native_source = include_str!("agent/native.rs");
        let inspection_source = include_str!("tests/inspection.rs");
        let legacy_agent_slot = ["native", "_agent_runtime"].concat();
        for runtime_source in [source, inspection_source] {
            for legacy_projection in [
                format!("\"{legacy_agent_slot}\""),
                format!("`{legacy_agent_slot}`"),
            ] {
                assert!(
                    !runtime_source.contains(&legacy_projection),
                    "runtime source must not retain legacy Agent slot {legacy_projection}"
                );
            }
        }
        let concrete_owner = ["agent_runtime: ", "NativeAgentRuntime"].concat();
        let concrete_constructor = ["NativeAgentRuntime", "::new("].concat();
        let plugin_factory_wiring = [
            "builtin_plugin_catalog_with_agent_factory(AgentRuntimeFactory::new(\n",
            "        construct_native_agent_runtime,\n",
            "    ))",
        ]
        .concat();
        let native_factory_definition = ["fn construct_native_", "agent_runtime("].concat();
        let native_construction = ["NativeAgentRuntime", "::new(mount)"].concat();
        let erased_native_owner = ["Box::new(runtime) as Box<dyn Agent", "RuntimePort>"].concat();
        let concrete_materialization = ["Self", " {"].concat();
        let plugin_factory_dispatch = [".construct_agent_", "runtime(mount)"].concat();
        let optional_factory = ["Option<Agent", "RuntimeFactory>"].concat();
        let old_mount_check = ["native_mount", "_check"].concat();
        let old_fresh_helper = ["fresh_native_", "agent_runtime"].concat();
        let old_replacement_guard = [
            "Agent plugin identity replacement requires an adapter ",
            "replacement transaction",
        ]
        .concat();
        let recording_port_source = source
            .split_once("impl AgentRuntimePort for RecordingAgentRuntime {")
            .and_then(|(_, tail)| tail.split_once("impl Drop for RecordingAgentRuntime"))
            .map(|(implementation, _)| implementation)
            .expect("recording Agent port implementation should remain inspectable");
        for removed_stub in [
            "fn session_id(",
            "fn is_history_empty(",
            "fn context_budget_snapshot(",
            "fn restore_session(",
        ] {
            assert!(
                !recording_port_source.contains(removed_stub),
                "recording Agent must not recreate removed stub {removed_stub}"
            );
        }
        let replay_factory_start = ["fn replay_agent_", "replacement_factory("].concat();
        let replay_factory_end = ["fn desired_with_", "agent_plugin_for_test("].concat();
        let replay_factory_source = source
            .split_once(&replay_factory_start)
            .and_then(|(_, tail)| tail.split_once(&replay_factory_end))
            .map(|(factory, _)| factory)
            .expect("Replay Agent factory should remain inspectable");
        assert!(replay_factory_source.contains("agent_runtime_plugin_factory("));
        assert!(replay_factory_source.contains(".requires(RUNTIME_EVENT_STREAM.capability)"));
        for forbidden in [
            "NativeAgentRuntime",
            "construct_native_agent_runtime",
            "components.agent_runtime =",
            "downcast",
        ] {
            assert!(
                !replay_factory_source.contains(forbidden),
                "Replay factory must not bypass plugin ownership through {forbidden}"
            );
        }

        assert_eq!(
            production_source
                .matches("agent_runtime: Box<dyn AgentRuntimePort>")
                .count(),
            1
        );
        assert!(!production_source.contains(&concrete_owner));
        assert!(!production_source.contains(&concrete_constructor));
        assert_eq!(production_source.matches(&plugin_factory_wiring).count(), 1);
        assert_eq!(
            production_source.matches(&plugin_factory_dispatch).count(),
            1
        );
        assert!(!production_source.contains(&optional_factory));
        assert!(!production_source.contains(&old_mount_check));
        assert!(!production_source.contains(&old_fresh_helper));
        assert!(!production_source.contains(&old_replacement_guard));
        assert_eq!(native_source.matches(&native_factory_definition).count(), 1);
        assert_eq!(native_source.matches(&native_construction).count(), 1);
        assert_eq!(native_source.matches(&erased_native_owner).count(), 1);
        assert_eq!(native_source.matches(&concrete_materialization).count(), 1);
        assert_eq!(
            production_source
                .matches("activate: RuntimeComponents::activate_agent_runtime")
                .count(),
            1
        );
        assert_eq!(
            production_source
                .matches("quiesce: RuntimeComponents::quiesce_agent_runtime")
                .count(),
            1
        );
        for forbidden in [
            ["downcast", "_ref"].concat(),
            ["downcast", "_mut"].concat(),
            ["std::any", "::Any"].concat(),
            ["dyn", " Any"].concat(),
        ] {
            assert!(
                !production_source.contains(&forbidden),
                "Agent owner must not regain concrete access through {forbidden}"
            );
        }
        let concrete_binding = ["agent_runtime", ".bind_event_stream"].concat();
        assert!(!production_source.contains(&concrete_binding));
        assert_eq!(
            production_source
                .matches("self.agent_runtime.activate(event_stream)?")
                .count(),
            1
        );
        let agent_quiescence_source = production_source
            .split_once("fn quiesce_agent_runtime(")
            .and_then(|(_, tail)| tail.split_once("fn finalize_agent_runtime_replacement("))
            .map(|(method, _)| method)
            .expect("Agent quiescence must remain host-owned");
        assert_eq!(agent_quiescence_source.matches(".suspend()").count(), 1);
        assert_eq!(agent_quiescence_source.matches(".shutdown()").count(), 1);

        let agent_finalization_source = production_source
            .split_once("fn finalize_agent_runtime_replacement(")
            .and_then(|(_, tail)| tail.split_once("fn quiesce_model_refresh("))
            .map(|(method, _)| method)
            .expect("Agent replacement must retain a mandatory finalization stage");
        assert_eq!(agent_finalization_source.matches(".shutdown()").count(), 1);

        let prepare_authority_source = production_source
            .split_once("fn prepare_authority(&mut self) -> Result<(), String> {")
            .and_then(|(_, tail)| tail.split_once("fn commit_authority(&mut self) {"))
            .map(|(method, _)| method)
            .expect("authority preparation should remain a distinct callback stage");
        assert_eq!(
            prepare_authority_source
                .matches(".prospective_implementation(")
                .count(),
            1
        );
        assert_eq!(
            prepare_authority_source
                .matches("construct_agent_runtime(implementation, mount)?")
                .count(),
            1
        );
        let finalization_position = prepare_authority_source
            .find("self.finalize_agent_runtime_replacement()?")
            .expect("authority preparation must finalize the old Agent");
        let construction_position = prepare_authority_source
            .find("construct_agent_runtime(implementation, mount)?")
            .expect("authority preparation must construct the fresh Agent");
        assert!(
            finalization_position < construction_position,
            "old Agent finalization must precede candidate construction"
        );

        let commit_authority_source = production_source
            .split_once("fn commit_authority(&mut self) {")
            .and_then(|(_, tail)| tail.split_once("fn abort_authority(&mut self) {"))
            .map(|(method, _)| method)
            .expect("authority commit should remain a distinct callback stage");
        assert!(!commit_authority_source.contains("construct_agent_runtime("));
        for publication in [
            "self.plugins.commit_reconciliation(prepared.reconciliation)",
            "self.plugin_loader.commit_desired(prepared.desired)",
            "self.agent_runtime = candidate",
        ] {
            assert!(
                commit_authority_source.contains(publication),
                "authority commit must publish {publication}"
            );
        }
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
            Ok(RuntimePluginImplementation::component(
                RuntimeComponents::activate_runtime_event_stream,
                RuntimeComponents::quiesce_noop,
            ))
        })
    }

    fn agent_replacement_factory(
        plugin_type: &'static str,
        factory: AgentRuntimeFactory,
    ) -> PluginFactory<RuntimePluginImplementation> {
        agent_runtime_plugin_factory(
            agent_runtime_descriptor(plugin_type, "Replacement Agent loop"),
            factory,
        )
    }

    fn traced_agent_replacement_factory(
        plugin_type: &'static str,
        factory: AgentRuntimeFactory,
        trace: Arc<Mutex<Vec<String>>>,
    ) -> PluginFactory<RuntimePluginImplementation> {
        let descriptor = agent_runtime_descriptor(plugin_type, "Traced replacement Agent loop")
            .build()
            .expect("traced Agent descriptor should validate");
        let implementation = RuntimePluginImplementation::agent_runtime(factory);
        PluginFactory::new(descriptor, move || {
            trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("prepare:{plugin_type}"));
            Ok(implementation.clone())
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
                (AGENT_RUNTIME_COMPONENT, NATIVE_AGENT_RUNTIME_PLUGIN),
                (APPROVAL_PROVIDER.component_id, APPROVAL_PROVIDER_PLUGIN),
                (CONTEXT_BUDGET_COMPONENT, CONTEXT_BUDGET_PLUGIN),
                (LLM_PORT.component_id, LLM_PORT_PLUGIN),
                (MODEL_REFRESH_COMPONENT, MODEL_REFRESH_PLUGIN),
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
                ComponentDefinition::new(AGENT_RUNTIME_COMPONENT)
                    .implemented_by(NATIVE_AGENT_RUNTIME_PLUGIN)
                    .requires(RUNTIME_EVENT_STREAM.capability)
                    .requires(LLM_PORT.capability)
                    .requires(MODEL_CATALOG.capability)
                    .requires(PERMISSION_POLICY.capability)
                    .requires(PROMPT_ASSEMBLY.capability)
                    .requires(TOOL_CATALOG.capability)
                    .observes(SESSION_PERSISTENCE.capability),
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
    fn external_extension_mount_is_dependency_bound_and_reversible() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        assert_eq!(components.external_extension_state(), None);
        assert!(
            components
                .plugin_descriptor_snapshots()
                .iter()
                .all(|snapshot| snapshot.component_id != EXTERNAL_EXTENSION_COMPONENT)
        );

        let first_transport = StaticExtensionTransport::default();
        let first_shutdowns = Arc::clone(&first_transport.shutdowns);
        components
            .mount_extension_tool_set(discovered_extension_set(first_transport))
            .expect("extension should activate after tool catalog is available");
        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Active)
        );
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .any(|definition| definition.name == "extension_echo")
        );

        let second_transport = StaticExtensionTransport::default();
        let second_shutdowns = Arc::clone(&second_transport.shutdowns);
        components
            .mount_extension_tool_set(discovered_extension_set(second_transport))
            .expect("a fresh mount should replace the old generation");
        assert_eq!(first_shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(second_shutdowns.load(Ordering::SeqCst), 0);
        assert_eq!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .filter(|definition| definition.name == "extension_echo")
                .count(),
            1
        );

        components
            .remove_extension_tool_set()
            .expect("remove should dispose extension effects");
        assert_eq!(components.external_extension_state(), None);
        assert_eq!(second_shutdowns.load(Ordering::SeqCst), 1);
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .all(|definition| definition.name != "extension_echo")
        );
    }

    #[test]
    fn external_extension_rediscoveries_after_tool_catalog_generation_reset() {
        let mut options = options_with_provider();
        let source = Arc::new(StaticExtensionSource::default());
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .mount_extension_tool_set(source.discover().expect("initial discovery should succeed"))
            .expect("extension should activate from a source-backed set");
        assert_eq!(source.discoveries.load(Ordering::SeqCst), 1);

        components
            .reset_after_clear(&options)
            .expect("catalog generation reset should restore the extension");

        assert_eq!(source.discoveries.load(Ordering::SeqCst), 2);
        assert_eq!(source.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .filter(|definition| definition.name == "extension_echo")
                .count(),
            1
        );
    }

    #[test]
    fn external_extension_becomes_pending_without_tool_catalog_and_does_not_rediscover() {
        let mut options = options_with_provider();
        let source = Arc::new(StaticExtensionSource::default());
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .mount_extension_tool_set(source.discover().expect("initial discovery should succeed"))
            .expect("extension should activate from a source-backed set");

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.remove_capability(
                    TOOL_CATALOG.component_id,
                    &CapabilityKey::from(TOOL_CATALOG.capability),
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("catalog removal should quiesce its dependent closure");

        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Pending)
        );
        assert_eq!(source.discoveries.load(Ordering::SeqCst), 1);
        assert_eq!(source.shutdowns.load(Ordering::SeqCst), 1);
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .all(|definition| definition.name != "extension_echo")
        );
    }

    #[test]
    fn agent_plugin_add_and_remove_are_rejected_before_runtime_mutation() {
        let agent_type = builtin_plugin_type(NATIVE_AGENT_RUNTIME_PLUGIN);
        assert_eq!(
            classify_agent_plugin_reconciliation(None, Some(&agent_type))
                .expect_err("Agent Add needs an absent-slot product contract"),
            "Runtime composition must contain exactly one Agent plugin"
        );
        assert_eq!(
            classify_agent_plugin_reconciliation(Some(&agent_type), None)
                .expect_err("Agent Remove needs an absent-slot product contract"),
            "Runtime composition must contain exactly one Agent plugin"
        );

        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_descriptors = components.plugin_descriptor_snapshots();
        let old_desired = components.plugin_loader.desired().clone();
        let old_components = components.lifecycle.components();
        let old_capabilities = components.lifecycle.capabilities();
        let old_context = components.lifecycle.context_snapshots();
        let old_scopes = components.effect_scope_snapshots();

        let error = components
            .reconcile_plugin_composition(
                &options,
                desired_with_agent_plugin_for_test(None),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("Agent Remove must fail before runtime mutation");

        assert_eq!(
            error,
            "Runtime composition must contain exactly one Agent plugin"
        );
        assert_eq!(components.plugin_descriptor_snapshots(), old_descriptors);
        assert_eq!(components.plugin_loader.desired(), &old_desired);
        assert_eq!(components.lifecycle.components(), old_components);
        assert_eq!(components.lifecycle.capabilities(), old_capabilities);
        assert_eq!(components.lifecycle.context_snapshots(), old_context);
        assert_eq!(components.effect_scope_snapshots(), old_scopes);
    }

    #[test]
    fn agent_plugin_replacement_constructs_after_cleanup_and_commits_matching_authority() {
        const ALTERNATE_AGENT: &str = "alternate-agent-loop";
        let mut options = AppRuntimeOptions::default();
        let old_lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let old_factory_trace = Arc::clone(&old_lifecycle_trace);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |_mount| {
                Ok(Box::new(RecordingAgentRuntime::new(Arc::clone(
                    &old_factory_trace,
                ))))
            }),
        )
        .expect("runtime components should initialize through the old Agent plugin");
        let transaction_trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&transaction_trace));
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_lifecycle_trace = Arc::clone(&lifecycle_trace);
        let factory_transaction_trace = Arc::clone(&transaction_trace);
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let catalog = PluginFactoryCatalog::try_new([traced_agent_replacement_factory(
            ALTERNATE_AGENT,
            AgentRuntimeFactory::new(move |_mount| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                factory_transaction_trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("construct:{ALTERNATE_AGENT}"));
                Ok(Box::new(RecordingAgentRuntime::new(Arc::clone(
                    &factory_lifecycle_trace,
                ))))
            }),
            Arc::clone(&transaction_trace),
        )])
        .expect("replacement Agent catalog should validate");

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("Agent replacement should commit and activate");
        components.plugin_transaction_trace = None;

        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert_eq!(
            *old_lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate", "suspend", "shutdown"],
            "Agent replacement must finally dispose the old adapter before construction"
        );
        assert_eq!(
            *transaction_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [
                format!("prepare:{ALTERNATE_AGENT}"),
                format!("quiesce:{AGENT_RUNTIME_COMPONENT}"),
                format!("construct:{ALTERNATE_AGENT}"),
                "prepare:authority".to_string(),
                "authority:commit".to_string(),
                format!("activate:{AGENT_RUNTIME_COMPONENT}"),
            ]
        );
        assert_eq!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate"]
        );
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("Agent descriptor should remain visible")
                .plugin_type,
            ALTERNATE_AGENT
        );
        assert_eq!(
            components
                .plugin_loader
                .desired()
                .iter()
                .find(|(component_id, _)| { component_id.as_str() == AGENT_RUNTIME_COMPONENT })
                .expect("desired composition should retain Agent slot")
                .1
                .as_str(),
            ALTERNATE_AGENT
        );
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        components
            .validate_context_alignment()
            .expect("fresh Agent plugin, graph, and Context should align");
    }

    #[test]
    fn replay_agent_uses_the_plugin_owner_and_reactive_lifecycle() {
        const REPLAY_AGENT: &str = "replay-agent-loop";
        let replay_fixture = ReplayFixture::new(vec![
            AgentEventKind::AssistantDelta {
                content: "replayed through composition".to_string(),
            },
            AgentEventKind::TurnFinished {
                response: runtime_domain::session::ConversationResponse::assistant_text(
                    "replayed through composition",
                ),
                metrics: None,
                context_usage: None,
            },
        ])
        .expect("Replay fixture should validate");
        let replay_lifecycle = Arc::new(ReplayLifecycleProbe::default());
        let catalog = PluginFactoryCatalog::try_new([replay_agent_replacement_factory(
            REPLAY_AGENT,
            replay_fixture,
            Some(Arc::clone(&replay_lifecycle)),
        )])
        .expect("Replay Agent catalog should validate");
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(REPLAY_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("Replay Agent should replace Native through plugin reconciliation");

        let agent = components
            .lifecycle
            .components()
            .into_iter()
            .find(|component| component.id == AGENT_RUNTIME_COMPONENT)
            .expect("Agent component should remain declared");
        assert_eq!(agent.required, [RUNTIME_EVENT_STREAM.capability]);
        assert!(agent.optional.is_empty());
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert_eq!(replay_lifecycle.constructions(), 1);
        assert_eq!(replay_lifecycle.activations(), 1);
        let restore_materialized = Arc::new(AtomicBool::new(false));
        let session_id = session_store::SessionId::new();
        let restore = AgentSessionRestore::new(
            Arc::new(session_store::InMemorySessionStore::new()),
            session_store::SessionHeader {
                session_id: session_id.clone(),
                work_dir: std::path::PathBuf::from("/replay-unavailable-restore"),
                session_name: None,
                initial_model: "fixture-model".to_string(),
                git_head: None,
                cli_version: None,
            },
            session_id,
            session_store::ResolvedConversationState {
                items: Vec::new(),
                latest_config: None,
            },
        )
        .with_materialization_probe(Arc::clone(&restore_materialized));
        let unavailable = match components
            .agent_session_mut()
            .and_then(|session| session.restore_session(restore))
        {
            Ok(()) => panic!("Replay must not accept a session restore"),
            Err(error) => error,
        };
        assert_eq!(
            unavailable,
            "Agent adapter does not provide session capability"
        );
        assert!(!restore_materialized.load(Ordering::SeqCst));
        assert_eq!(
            components.agent_port().activity(),
            AgentRuntimeActivity::Idle
        );
        assert!(!components.agent_port().has_pending_work());
        assert!(components.agent_port_mut().drain_events().is_empty());

        let target = runtime_domain::session::RuntimeTarget::provider("replay", "fixture-model");
        components
            .agent_port_mut()
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: AgentTurnId::new(1),
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    runtime_domain::session::ConversationTurnRequest::new(
                        "replay",
                        "fixture-model",
                        provider_protocol::ConversationItem::text(
                            provider_protocol::Role::User,
                            "delivery must not be projected by the plugin host",
                        ),
                    ),
                )),
            })
            .expect("Replay turn should be admitted through the common owner");
        let projected = components
            .agent_port_mut()
            .drain_events()
            .into_iter()
            .map(crate::runtime::event_mapping::runtime_event_from_agent_event)
            .collect::<Vec<_>>();
        assert!(matches!(
            &projected[0],
            runtime_domain::session::RuntimeEvent::AssistantDelta {
                target: event_target,
                content,
            } if event_target == &target && content == "replayed through composition"
        ));
        assert!(matches!(
            &projected[1],
            runtime_domain::session::RuntimeEvent::MessageFinished {
                target: Some(event_target),
                ..
            } if event_target == &target
        ));

        components
            .agent_port_mut()
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: AgentTurnId::new(2),
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    runtime_domain::session::ConversationTurnRequest::new(
                        "replay",
                        "fixture-model",
                        provider_protocol::ConversationItem::text(
                            provider_protocol::Role::User,
                            "old generation",
                        ),
                    ),
                )),
            })
            .expect("second Replay turn should queue facts before dependency suspension");
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("event-stream suspension should quiesce Replay");
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Pending)
        );
        assert!(components.agent_port_mut().drain_events().is_empty());
        assert!(!components.agent_port().has_pending_work());

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.activate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("fresh event-stream generation should reactivate Replay");
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert_eq!(replay_lifecycle.constructions(), 1);
        assert_eq!(replay_lifecycle.activations(), 2);

        let native_catalog = builtin_plugin_catalog().expect("Native catalog should validate");
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &native_catalog,
                desired_with_agent_plugin_for_test(Some(NATIVE_AGENT_RUNTIME_PLUGIN)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("Native Agent should replace Replay through the same transaction");
        assert_eq!(replay_lifecycle.shutdowns(), 1);
        assert_eq!(replay_lifecycle.drops(), 1);
        assert_eq!(replay_lifecycle.constructions(), 1);
        assert!(components.agent_session().is_ok());
        assert!(components.agent_port_mut().drain_events().is_empty());
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("Agent descriptor should remain visible")
                .plugin_type,
            NATIVE_AGENT_RUNTIME_PLUGIN
        );
        components
            .validate_context_alignment()
            .expect("Native restoration should keep graph and Context aligned");
    }

    #[test]
    fn agent_plugin_cleanup_failure_aborts_before_candidate_construction() {
        const ALTERNATE_AGENT: &str = "alternate-agent-loop";
        let mut options = AppRuntimeOptions::default();
        let old_lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let startup_trace = Arc::clone(&old_lifecycle_trace);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |_mount| {
                Ok(Box::new(RecordingAgentRuntime::with_shutdown_failure(
                    Arc::clone(&startup_trace),
                )))
            }),
        )
        .expect("runtime should initialize through the injected Agent plugin factory");
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let catalog = PluginFactoryCatalog::try_new([agent_replacement_factory(
            ALTERNATE_AGENT,
            AgentRuntimeFactory::new(move |_mount| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(RecordingAgentRuntime::new(Arc::new(Mutex::new(
                    Vec::new(),
                )))))
            }),
        )])
        .expect("replacement Agent catalog should validate");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("old Agent cleanup failure must block candidate construction");

        assert_eq!(constructions.load(Ordering::SeqCst), 0);
        assert!(!error.contains("SENSITIVE_AGENT_CLEANUP_FAILURE"));
        assert_eq!(
            *old_lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate", "suspend", "shutdown"]
        );
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("old Agent descriptor should remain visible")
                .plugin_type,
            NATIVE_AGENT_RUNTIME_PLUGIN
        );
        assert_ne!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );

        let retry_error = components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("retry must not bypass incomplete old Agent finalization");
        assert!(!retry_error.contains("SENSITIVE_AGENT_CLEANUP_FAILURE"));
        assert_eq!(constructions.load(Ordering::SeqCst), 0);
        assert_eq!(
            *old_lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate", "suspend", "shutdown", "shutdown"]
        );
    }

    #[test]
    fn stale_graph_commit_drops_the_actual_agent_replacement_transaction() {
        const ALTERNATE_AGENT: &str = "alternate-agent-stale";
        let mut options = AppRuntimeOptions::default();
        let observer = Arc::clone(&options.dynamic_environment_observer);
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let observer_owner_count = Arc::strong_count(&observer);
        let old_descriptors = components.plugin_descriptor_snapshots();
        let old_desired = components.plugin_loader.desired().clone();
        let candidate_drops = Arc::new(AtomicUsize::new(0));
        let implementation_drops = Arc::new(AtomicUsize::new(0));
        let prepared_candidate_drops = Arc::clone(&candidate_drops);
        let prepared_implementation_drops = Arc::clone(&implementation_drops);
        let catalog = PluginFactoryCatalog::try_new([PluginFactory::new(
            agent_runtime_descriptor(ALTERNATE_AGENT, "Stale replacement Agent")
                .build()
                .expect("replacement Agent descriptor should validate"),
            move || {
                let implementation_drop = DropCounter {
                    count: Arc::clone(&prepared_implementation_drops),
                };
                let constructor_candidate_drops = Arc::clone(&prepared_candidate_drops);
                Ok(RuntimePluginImplementation::agent_runtime(
                    AgentRuntimeFactory::new(move |mount| {
                        let _implementation_owner = &implementation_drop;
                        Ok(Box::new(RecordingAgentRuntime::with_abort_probes(
                            Arc::new(Mutex::new(Vec::new())),
                            mount,
                            Arc::clone(&constructor_candidate_drops),
                        )))
                    }),
                ))
            },
        )])
        .expect("replacement Agent catalog should validate");
        let desired = desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT));
        let agent_runtime = components
            .prepare_agent_runtime_commit(&options, &desired)
            .expect("Agent replacement mount should prepare");
        assert_eq!(Arc::strong_count(&observer), observer_owner_count + 1);
        let reconciliation = catalog
            .prepare_reconciliation(&desired, &components.plugins)
            .expect("fresh Agent plugin should prepare");
        let prepared_graph = components
            .lifecycle
            .prepare_definition_reconciliation(reconciliation.definitions())
            .expect("replacement graph should preflight");
        let retirement_order = prepared_graph.retirement_order().to_vec();
        components.prepared_plugin_commit = Some(PreparedPluginCommit {
            desired,
            reconciliation,
            agent_runtime,
        });
        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    retirement_order,
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("old Agent effects should retire before candidate construction");
        components
            .prepare_authority()
            .expect("fresh Agent adapter should prepare after cleanup");

        assert_eq!(candidate_drops.load(Ordering::SeqCst), 0);
        assert_eq!(implementation_drops.load(Ordering::SeqCst), 0);
        assert_eq!(Arc::strong_count(&observer), observer_owner_count + 1);
        components
            .lifecycle
            .declare(ComponentDefinition::new("unexpected_component"))
            .expect("test mutation should stale the prepared graph");

        let error = components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.commit_prepared_graph_for_test(prepared_graph, components)
            })
            .expect_err("stale graph commit must abort the real host transaction");

        assert!(!format!("{error:?}").contains(ALTERNATE_AGENT));
        assert!(components.prepared_plugin_commit.is_none());
        assert_eq!(candidate_drops.load(Ordering::SeqCst), 1);
        assert_eq!(implementation_drops.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&observer), observer_owner_count);
        assert_eq!(components.plugin_descriptor_snapshots(), old_descriptors);
        assert_eq!(components.plugin_loader.desired(), &old_desired);
    }

    #[test]
    fn failed_agent_plugin_construction_keeps_old_authority_and_retry_can_commit() {
        const ALTERNATE_AGENT: &str = "alternate-agent-loop";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_desired = components.plugin_loader.desired().clone();
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_lifecycle_trace = Arc::clone(&lifecycle_trace);
        let catalog = PluginFactoryCatalog::try_new([agent_replacement_factory(
            ALTERNATE_AGENT,
            AgentRuntimeFactory::new(move |_mount| {
                if observed_constructions.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err("SENSITIVE_AGENT_CONSTRUCTION_FAILURE".to_string())
                } else {
                    Ok(Box::new(RecordingAgentRuntime::new(Arc::clone(
                        &factory_lifecycle_trace,
                    ))))
                }
            }),
        )])
        .expect("replacement Agent catalog should validate");
        let desired = desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT));

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired.clone(),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("first Agent candidate construction should fail");

        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert!(error.contains("authority_preparation"));
        assert!(!error.contains("SENSITIVE_AGENT_CONSTRUCTION_FAILURE"));
        assert_eq!(components.plugin_loader.desired(), &old_desired);
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("old Agent descriptor should remain visible")
                .plugin_type,
            NATIVE_AGENT_RUNTIME_PLUGIN
        );
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Pending),
            "old Agent effects must stay removed after candidate failure"
        );

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("retry should construct and publish the fresh Agent");

        assert_eq!(constructions.load(Ordering::SeqCst), 2);
        assert_eq!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate"]
        );
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("fresh Agent descriptor should be visible")
                .plugin_type,
            ALTERNATE_AGENT
        );
    }

    #[test]
    fn failed_fresh_agent_activation_keeps_fresh_plugin_and_adapter_authority() {
        const ALTERNATE_AGENT: &str = "alternate-agent-rejected";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let observed_shutdown_calls = Arc::clone(&shutdown_calls);
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_lifecycle_trace = Arc::clone(&lifecycle_trace);
        let catalog = PluginFactoryCatalog::try_new([agent_replacement_factory(
            ALTERNATE_AGENT,
            AgentRuntimeFactory::new(move |_mount| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(RecordingAgentRuntime::with_shutdown_counter(
                    Arc::clone(&factory_lifecycle_trace),
                    Arc::clone(&observed_shutdown_calls),
                )))
            }),
        )])
        .expect("replacement Agent catalog should validate");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("fresh Agent activation rejection should be reported");

        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert!(!error.contains("SENSITIVE_AGENT_ACTIVATION_FAILURE"));
        assert_eq!(
            components
                .plugin_descriptor_snapshots()
                .into_iter()
                .find(|snapshot| snapshot.component_id == AGENT_RUNTIME_COMPONENT)
                .expect("fresh Agent descriptor should remain authoritative")
                .plugin_type,
            ALTERNATE_AGENT
        );
        assert_eq!(
            components
                .plugin_loader
                .desired()
                .iter()
                .find(|(component_id, _)| { component_id.as_str() == AGENT_RUNTIME_COMPONENT })
                .expect("fresh desired Agent should remain committed")
                .1
                .as_str(),
            ALTERNATE_AGENT
        );
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Failed)
        );
        assert_eq!(
            shutdown_calls.load(Ordering::SeqCst),
            1,
            "failed fresh activation must finally dispose the unpublished effects of its adapter"
        );
        assert!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                == ["shutdown"],
            "rejected activation must finally dispose the candidate adapter"
        );
        assert!(components.agent_port().session().is_none());
    }

    #[test]
    fn repeated_shutdown_never_reports_success_after_incomplete_agent_cleanup() {
        let mut options = AppRuntimeOptions::default();
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_trace = Arc::clone(&lifecycle_trace);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |_mount| {
                Ok(Box::new(RecordingAgentRuntime::with_shutdown_failure(
                    Arc::clone(&factory_trace),
                )))
            }),
        )
        .expect("runtime should activate through the failing shutdown fixture");

        let first = components
            .shutdown()
            .expect_err("incomplete Agent cleanup must fail shutdown");
        let second = components
            .shutdown()
            .expect_err("repeated shutdown must preserve the cleanup failure");

        assert!(!first.contains("SENSITIVE_AGENT_CLEANUP_FAILURE"));
        assert!(!second.contains("SENSITIVE_AGENT_CLEANUP_FAILURE"));
        assert!(components.is_shutdown);
        let cached_lifecycle_error = components
            .shutdown_lifecycle_error
            .as_deref()
            .expect("first shutdown failure must remain observable");
        assert!(first.contains(cached_lifecycle_error));
        assert!(second.contains(cached_lifecycle_error));
        assert_eq!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate", "shutdown", "shutdown", "shutdown"]
        );
    }

    #[test]
    fn agent_plugin_keep_reuses_the_current_adapter_during_dependency_replacement() {
        const FRESH_EVENT_PLUGIN: &str = "runtime-event-stream-agent-keep";
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let mut options = AppRuntimeOptions::default();
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |mount| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                construct_native_agent_runtime(mount)
            }),
        )
        .expect("runtime should initialize through the injected Agent plugin factory");
        let catalog = PluginFactoryCatalog::try_new([runtime_event_replacement_factory(
            FRESH_EVENT_PLUGIN,
            RuntimeComponents::activate_runtime_event_stream,
        )])
        .expect("replacement runtime event catalog should validate");

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_runtime_event_plugin(FRESH_EVENT_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("dependency replacement should reactivate the kept Agent");

        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
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
                &options,
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
                    &options,
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
                    &options,
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
                &options,
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
                &options,
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
                &options,
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
                &options,
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
                &options,
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
            components.lifecycle.state("agent_runtime"),
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
            AGENT_RUNTIME_COMPONENT,
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
        assert!(!components.agent_port().activity().is_busy());
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
            AGENT_RUNTIME_COMPONENT,
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
            components
                .lifecycle
                .optional_available("agent_runtime", &CapabilityKey::from("session_persistence"),),
            Some(false)
        );
        assert_eq!(
            components.lifecycle.state("agent_runtime"),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state("prompt_assembly"),
            Some(ComponentState::Active)
        );
        let snapshot = components
            .agent_session()
            .expect("Native Agent should provide session capability")
            .snapshot();
        assert!(!components.agent_port().activity().is_busy());
        assert!(snapshot.is_history_empty);
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
    fn failed_agent_plugin_mount_reverts_fresh_provider_prompt_and_tool_effects() {
        let mut options = AppRuntimeOptions {
            loaded_models: options_with_provider().loaded_models,
            initial_prompt_assembly: Some(manager_with_section("initial", "initial body")),
            ..AppRuntimeOptions::default()
        };
        let construction_attempts = Arc::new(AtomicUsize::new(0));
        let factory_attempts = Arc::clone(&construction_attempts);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |mount| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    construct_native_agent_runtime(mount)
                } else {
                    Err("injected Agent plugin mount failure".to_string())
                }
            }),
        )
        .expect("runtime components should initialize through the injected plugin factory");

        let error = components
            .reset_after_clear(&options)
            .expect_err("injected Agent plugin mount failure should abort publication");

        assert_eq!(error, "Agent plugin failed to construct its adapter");
        assert!(!error.contains("injected Agent plugin mount failure"));
        assert_eq!(construction_attempts.load(Ordering::SeqCst), 2);
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
            components.lifecycle.state("agent_runtime"),
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
            components.lifecycle.state("agent_runtime"),
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
            components.lifecycle.state("agent_runtime"),
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
            components.lifecycle.state("agent_runtime"),
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
        let construction_attempts = Arc::new(AtomicUsize::new(0));
        let factory_attempts = Arc::clone(&construction_attempts);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |mount| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 1 {
                    Err("injected session Agent plugin mount failure".to_string())
                } else {
                    construct_native_agent_runtime(mount)
                }
            }),
        )
        .expect("runtime components should initialize through the injected plugin factory");
        assert!(options.session_store.is_none());
        assert!(components.session_backend_views.is_some());
        assert!(
            components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );
        let error = components
            .replace_session_backend(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
            )
            .expect_err("injected session backend mount failure should abort publication");

        assert_eq!(error, "Agent plugin failed to construct its adapter");
        assert!(!error.contains("injected session Agent plugin mount failure"));
        assert_eq!(construction_attempts.load(Ordering::SeqCst), 3);
        assert!(components.session_backend_views.is_none());
        assert!(
            !components
                .lifecycle
                .has_capability(&CapabilityKey::from("session_persistence"))
        );
        let snapshot = components
            .agent_session()
            .expect("Native Agent should provide session capability")
            .snapshot();
        assert!(!components.agent_port().activity().is_busy());
        assert!(snapshot.is_history_empty);
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
            components.lifecycle.state("agent_runtime"),
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
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
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
        let construction_attempts = Arc::new(AtomicUsize::new(0));
        let factory_attempts = Arc::clone(&construction_attempts);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |mount| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    construct_native_agent_runtime(mount)
                } else {
                    Err("injected permission Agent plugin mount failure".to_string())
                }
            }),
        )
        .expect("runtime components should initialize through the injected plugin factory");

        let error = components
            .replace_permission_provider(
                &options,
                "replacement-owner",
                "replacement-provider",
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .expect_err("injected Agent plugin mount failure should abort replacement");

        assert_eq!(error, "Agent plugin failed to construct its adapter");
        assert!(!error.contains("injected permission Agent plugin mount failure"));
        assert_eq!(construction_attempts.load(Ordering::SeqCst), 2);
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
            components.lifecycle.state("agent_runtime"),
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
            components.lifecycle.state("agent_runtime"),
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
            .inject_epoch_exhaustion(AGENT_RUNTIME_COMPONENT);

        let error = components
            .replace_session_backend(
                &options,
                Arc::new(session_store::InMemorySessionStore::new()),
            )
            .expect_err("observed consumer exhaustion should reject replacement before cleanup");

        assert!(error.contains("component `agent_runtime` activation epoch is exhausted"));
        assert_eq!(components.lifecycle.capabilities(), capabilities_before);
        assert!(components.session_port.is_some());
        assert!(components.session_backend_views.is_some());
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert!(Arc::strong_count(&store) > 1);
    }
}
