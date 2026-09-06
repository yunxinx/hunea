use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

use conversation_runtime::ModelRefreshWorker;
use extension_hook_runtime::ExtensionHookRegistry;
use extension_runtime::{ExtensionBundle, ExtensionBundleSource, ExtensionMount};
use runtime_domain::event_notifier::{RuntimeEventBinding, RuntimeEventNotifier};
use tool_runtime::{ToolCatalog, ToolExecutorRegistry, ToolRegistration};

use super::{
    AppRuntimeOptions,
    agent::{
        AgentChildRuntimeLeases, AgentChildRuntimeStaticGrants, AgentRuntimeActivationGrants,
        AgentRuntimeConstructionGrants, AgentRuntimeFactory, AgentRuntimePort,
        AgentSessionCapability, SpawnAgentsRequest, SpawnAgentsTool,
        construct_native_agent_runtime, construct_native_child_agent_runtime,
    },
    agent_orchestrator::AgentOrchestrator,
    context::{
        ApprovalProviderCapability, CapabilityLease, ComponentActivationContext,
        ExtensionHookRegistryCapability, LlmPortCapability, ModelCatalogCapability,
        PermissionPolicyCapability, PromptAssemblyCapability, RuntimeCapability,
        RuntimeContextError, RuntimeEventStreamCapability, RuntimeWakeCapability,
        SessionPersistenceCapability, ToolCatalogCapability,
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
        PluginFactoryCatalog, PluginInstanceView, PluginReloadPolicy, PluginTrust, PluginTypeId,
        PreparedPluginReconciliation,
    },
    prompt_assembly::{PromptAssembly, PromptRegistration},
    session_port::{SessionBackendRegistration, SessionBackendViews, SessionPortHost},
    session_tools_for_manager,
    session_worker::SessionStoreWorker,
    workspace_tools::conversation_workspace_tool_catalog,
};
use runtime_domain::runtime_wake::RuntimeWake;
use tokio::sync::mpsc;

#[cfg(test)]
use super::agent::{ReplayFixture, ReplayLifecycleProbe};

#[cfg(test)]
use agent_kernel_runtime::{AgentKernelSource, ExternalAgentRuntimeOptions};

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
const EXTENSION_HOOKS: CapabilityOwner = CapabilityOwner {
    component_id: "extension_hooks",
    capability: "extension_hooks",
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
const EXTERNAL_EXTENSION_COMPONENT: &str = "externalextension";

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
const EXTERNAL_EXTENSION_PLUGIN: &str = "out-of-process-extension";
const EXTENSION_HOOKS_PLUGIN: &str = "typed-extension-hooks";

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
        descriptor: &PluginDescriptor,
        grant_source: AgentRuntimeGrantSource<'_>,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        match &self.kind {
            RuntimePluginImplementationKind::AgentRuntime(factory) => {
                factory.construct(grant_source.project(descriptor))
            }
            RuntimePluginImplementationKind::Component => {
                Err("plugin implementation does not provide an Agent runtime factory".to_string())
            }
        }
    }

    fn child_factory(&self) -> Option<super::agent::ChildAgentFactory> {
        match &self.kind {
            RuntimePluginImplementationKind::AgentRuntime(factory) => factory.child_factory(),
            RuntimePluginImplementationKind::Component => None,
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
        .requires(EXTENSION_HOOKS.capability)
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
        desired_builtin(EXTENSION_HOOKS.component_id, EXTENSION_HOOKS_PLUGIN),
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
fn external_agent_replacement_factory(
    plugin_type: &'static str,
    source: Arc<dyn AgentKernelSource>,
) -> PluginFactory<RuntimePluginImplementation> {
    agent_runtime_plugin_factory(
        builtin_descriptor(plugin_type, "External Agent kernel")
            .requires(RUNTIME_EVENT_STREAM.capability),
        AgentRuntimeFactory::external(source, ExternalAgentRuntimeOptions::default()),
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

#[cfg(test)]
fn desired_with_extension_hooks_plugin_for_test(
    current: &DesiredPluginComposition,
    plugin_type: &'static str,
) -> DesiredPluginComposition {
    DesiredPluginComposition::try_new(current.iter().map(|(component_id, current_type)| {
        if component_id.as_str() == EXTENSION_HOOKS.component_id {
            (component_id.as_str(), builtin_plugin_type(plugin_type))
        } else {
            (component_id.as_str(), current_type.clone())
        }
    }))
    .expect("extension hook replacement desired state should validate")
}

#[cfg(test)]
fn desired_without_extension_hooks_for_test(
    current: &DesiredPluginComposition,
) -> DesiredPluginComposition {
    DesiredPluginComposition::try_new(
        current
            .iter()
            .filter(|(component_id, _)| component_id.as_str() != EXTENSION_HOOKS.component_id)
            .map(|(component_id, current_type)| (component_id.as_str(), current_type.clone())),
    )
    .expect("extension hook removal desired state should validate")
}

#[cfg(test)]
fn extension_hooks_replacement_factory(
    plugin_type: &'static str,
) -> PluginFactory<RuntimePluginImplementation> {
    runtime_plugin_factory(
        builtin_descriptor(plugin_type, "Replacement typed extension hooks")
            .provides(EXTENSION_HOOKS.capability),
        RuntimeComponents::activate_extension_hooks,
        RuntimeComponents::quiesce_extension_hooks,
    )
}

fn builtin_plugin_catalog()
-> Result<PluginFactoryCatalog<RuntimePluginImplementation>, PluginCatalogError> {
    builtin_plugin_catalog_with_agent_factory(AgentRuntimeFactory::with_child_constructor(
        construct_native_agent_runtime,
        construct_native_child_agent_runtime,
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
            builtin_descriptor(EXTENSION_HOOKS_PLUGIN, "Typed extension hooks")
                .provides(EXTENSION_HOOKS.capability),
            RuntimeComponents::activate_extension_hooks,
            RuntimeComponents::quiesce_extension_hooks,
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
            builtin_descriptor(EXTERNAL_EXTENSION_PLUGIN, "External extension")
                .requires(TOOL_CATALOG.capability)
                .requires(EXTENSION_HOOKS.capability),
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
    extension_bundle: Option<ExtensionBundle>,
    extension_source: Option<Arc<dyn ExtensionBundleSource>>,
    is_session_backend_replacement: bool,
}

struct PreparedPluginCommit {
    desired: DesiredPluginComposition,
    reconciliation: PreparedPluginReconciliation<RuntimePluginImplementation>,
    agent_runtime: PreparedAgentRuntimeCommit,
}

enum PreparedAgentRuntimeCommit {
    Keep,
    ReplacePending,
    ReplaceReady(Box<PreparedAgentRuntimeReplacement>),
}

struct PreparedAgentRuntimeReplacement {
    candidate: Box<dyn AgentRuntimePort>,
    child_factory: Option<super::agent::ChildAgentFactory>,
    child_static_grants: Option<AgentChildRuntimeStaticGrants>,
    next_generation: runtime_domain::agent::AgentRuntimeGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentPluginReconciliation {
    Keep,
    Replace,
}

struct AgentRuntimeGrantSource<'a> {
    options: &'a AppRuntimeOptions,
    extension_hooks: &'a ExtensionHookRegistry,
    session_workspace_tools: &'a ToolExecutorRegistry,
    tool_catalog: &'a ToolCatalog,
    prompt_assembly: &'a PromptAssembly,
    session_port: Option<&'a Arc<dyn session_store::SessionPort>>,
    llm_port: &'a LlmPort,
    permission_policy: &'a PermissionPolicy,
    permission_provider_id: &'a str,
    #[cfg(test)]
    materialization_probe: Option<Arc<AgentRuntimeGrantMaterializationProbe>>,
}

struct PluginCommitCallbacks<'a> {
    components: &'a mut RuntimeComponents,
    agent_options: Option<&'a AppRuntimeOptions>,
}

#[cfg(test)]
#[derive(Default)]
struct AgentRuntimeGrantMaterializationProbe {
    materializations: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl AgentRuntimeGrantMaterializationProbe {
    fn record(&self) {
        self.materializations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn materializations(&self) -> usize {
        self.materializations
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl<'a> AgentRuntimeGrantSource<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        options: &'a AppRuntimeOptions,
        extension_hooks: &'a ExtensionHookRegistry,
        session_workspace_tools: &'a ToolExecutorRegistry,
        tool_catalog: &'a ToolCatalog,
        prompt_assembly: &'a PromptAssembly,
        session_port: Option<&'a Arc<dyn session_store::SessionPort>>,
        llm_port: &'a LlmPort,
        permission_policy: &'a PermissionPolicy,
        permission_provider_id: &'a str,
    ) -> Self {
        Self {
            options,
            extension_hooks,
            session_workspace_tools,
            tool_catalog,
            prompt_assembly,
            session_port,
            llm_port,
            permission_policy,
            permission_provider_id,
            #[cfg(test)]
            materialization_probe: None,
        }
    }

    #[cfg(test)]
    fn with_materialization_probe(
        mut self,
        probe: Option<Arc<AgentRuntimeGrantMaterializationProbe>>,
    ) -> Self {
        self.materialization_probe = probe;
        self
    }

    fn record_materialization(&self) {
        #[cfg(test)]
        if let Some(probe) = &self.materialization_probe {
            probe.record();
        }
    }

    fn project(self, descriptor: &PluginDescriptor) -> AgentRuntimeConstructionGrants {
        let mut grants = AgentRuntimeConstructionGrants::empty();
        if descriptor.declares_required(EXTENSION_HOOKS.capability) {
            self.record_materialization();
            grants = grants.with_extension_hooks(self.extension_hooks.clone());
        }
        if descriptor.declares_required(LLM_PORT.capability) {
            self.record_materialization();
            grants = grants.with_llm_port(self.llm_port.clone());
        }
        if descriptor.declares_required(MODEL_CATALOG.capability) {
            self.record_materialization();
            grants = grants.with_loaded_models(self.options.loaded_models.clone());
        }
        if descriptor.declares_required(PERMISSION_POLICY.capability) {
            self.record_materialization();
            grants = grants.with_permission(
                self.options.runtime_request_policy.clone(),
                self.permission_policy.clone(),
                self.permission_provider_id.to_string(),
            );
        }
        if descriptor.declares_required(PROMPT_ASSEMBLY.capability) {
            self.record_materialization();
            grants = grants.with_prompt(
                Arc::clone(&self.options.dynamic_environment_observer),
                self.options.hunea_config_dir.clone(),
                self.prompt_assembly.session_snapshot(),
            );
        }
        if descriptor.declares_required(TOOL_CATALOG.capability) {
            self.record_materialization();
            grants = grants.with_tools(
                self.session_workspace_tools.clone(),
                self.tool_catalog.definitions(),
            );
        }
        if descriptor.declares_observed(SESSION_PERSISTENCE.capability) {
            self.record_materialization();
            grants = grants.with_session(
                self.options.session_header_template.clone(),
                self.session_port.map(Arc::clone),
            );
        }
        grants
    }
}

/// `RuntimeComponents` 是 coordinator 的长期 runtime owner。
///
/// 它把 active Agent adapter、host workers、tool view、notifier 和 lifecycle graph 放在
/// 同一所有权边界，让 reset/shutdown 不再依赖 coordinator 手工枚举底层 conversation
/// resources。
pub(super) struct RuntimeComponents {
    agent_orchestrator: AgentOrchestrator,
    pub(super) model_refresh: ModelRefreshWorker,
    extension_hooks: ExtensionHookRegistry,
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
    extension_mount: Option<Arc<Mutex<ExtensionMount>>>,
    extension_source: Option<Arc<dyn ExtensionBundleSource>>,
    runtime_event_notifier: RuntimeEventNotifier,
    spawn_agents_receiver: mpsc::UnboundedReceiver<SpawnAgentsRequest>,
    spawn_agents_tool: SpawnAgentsTool,
    activation_staging: ComponentActivationStaging,
    plugin_loader: PluginCompositionLoader<RuntimePluginImplementation>,
    plugins: PluginComposition<RuntimePluginImplementation>,
    prepared_plugin_commit: Option<PreparedPluginCommit>,
    is_agent_replacement_activating: bool,
    #[cfg(test)]
    plugin_transaction_trace: Option<Arc<Mutex<Vec<String>>>>,
    #[cfg(test)]
    agent_grant_materialization_probe: Option<Arc<AgentRuntimeGrantMaterializationProbe>>,
    pub(super) lifecycle: ComponentLifecycleExecutor,
    finalization: RuntimeFinalization,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RuntimeFinalization {
    #[default]
    Open,
    Finalizing,
    Succeeded,
}

fn construct_agent_runtime(
    instance: PluginInstanceView<'_, RuntimePluginImplementation>,
    grant_source: AgentRuntimeGrantSource<'_>,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    instance
        .implementation()
        .construct_agent_runtime(instance.descriptor(), grant_source)
}

fn construct_committed_agent_runtime(
    plugins: &PluginComposition<RuntimePluginImplementation>,
    grant_source: AgentRuntimeGrantSource<'_>,
) -> Result<Box<dyn AgentRuntimePort>, String> {
    let instance = plugins
        .instance(AGENT_RUNTIME_COMPONENT)
        .ok_or_else(|| "Agent component has no committed plugin implementation".to_string())?;
    construct_agent_runtime(instance, grant_source)
}

fn committed_agent_child_factory(
    plugins: &PluginComposition<RuntimePluginImplementation>,
) -> Option<super::agent::ChildAgentFactory> {
    plugins
        .instance(AGENT_RUNTIME_COMPONENT)
        .and_then(|instance| instance.implementation().child_factory())
}

fn agent_child_static_grants(
    child_factory: Option<&super::agent::ChildAgentFactory>,
    options: &AppRuntimeOptions,
    permission_provider_id: &str,
) -> Option<AgentChildRuntimeStaticGrants> {
    child_factory.map(|_| {
        AgentChildRuntimeStaticGrants::new(
            options.runtime_request_policy.clone(),
            options.loaded_models.clone(),
            Arc::clone(&options.dynamic_environment_observer),
            options.hunea_config_dir.clone(),
            permission_provider_id,
        )
    })
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
    pub(super) fn dispatch_main_agent(
        &mut self,
        command: runtime_domain::agent::AgentCommand,
    ) -> Result<runtime_domain::agent::AgentCommandReceipt, runtime_domain::agent::AgentRuntimeError>
    {
        self.agent_orchestrator.dispatch_main(command)
    }

    pub(super) fn drain_main_agent_events(&mut self) -> Vec<runtime_domain::agent::AgentEvent> {
        self.agent_orchestrator.drain_main_events()
    }

    /// 在 runtime event consumer 边界推进 child projection；projection facts 由
    /// `drain_agent_projection_events` 取出并经 `RuntimeEvent::AgentProjection` 交付。
    pub(super) fn drain_child_agent_events(&mut self) -> Vec<runtime_domain::agent::AgentEvent> {
        self.agent_orchestrator.drain_child_events()
    }

    /// 消费 host-owned `spawn_agents` bridge；tool 自身不直接修改 orchestrator。
    pub(super) fn drain_spawn_agents_requests(&mut self) {
        while let Ok(request) = self.spawn_agents_receiver.try_recv() {
            self.agent_orchestrator.handle_spawn_agents_request(request);
        }
    }

    pub(super) fn rebind_main_tools(
        &mut self,
        registry: ToolExecutorRegistry,
    ) -> Result<(), String> {
        self.agent_orchestrator.rebind_main_tools(registry)
    }

    #[allow(dead_code)]
    pub(super) fn dispatch_child_agent(
        &mut self,
        command: runtime_domain::agent::AgentCommand,
    ) -> Result<runtime_domain::agent::AgentCommandReceipt, runtime_domain::agent::AgentRuntimeError>
    {
        self.agent_orchestrator.dispatch_child(command)
    }

    #[allow(dead_code)]
    pub(super) fn spawn_child_agent(
        &mut self,
        parent_agent_id: runtime_domain::agent::AgentId,
        turn_id: runtime_domain::agent::AgentTurnId,
        title: runtime_domain::agent::AgentTitle,
        grants: crate::runtime::agent_capability_context::AgentChildCapabilityGrants,
        request: runtime_domain::agent::AgentTurnRequest,
    ) -> Result<
        (
            runtime_domain::agent::AgentId,
            runtime_domain::agent::AgentCommandReceipt,
        ),
        runtime_domain::agent::AgentRuntimeError,
    > {
        self.agent_orchestrator
            .spawn_child(parent_agent_id, turn_id, title, grants, request)
    }

    #[allow(dead_code)]
    pub(super) fn stop_child_agent(
        &mut self,
        agent_id: runtime_domain::agent::AgentId,
    ) -> Result<(), runtime_domain::agent::AgentRuntimeError> {
        self.agent_orchestrator.stop_child(agent_id)
    }

    /// 建立一个 overview observation 并通过 runtime event notifier 唤醒 event pump。
    pub(super) fn observe_agents(
        &mut self,
        request_id: runtime_domain::agent::AgentObservationRequestId,
    ) {
        self.agent_orchestrator.observe_agents(request_id);
        self.notify_runtime_event();
    }

    /// 建立一个 per-agent observation 并通过 runtime event notifier 唤醒 event pump。
    pub(super) fn observe_agent_transcript(
        &mut self,
        request_id: runtime_domain::agent::AgentObservationRequestId,
        agent_id: runtime_domain::agent::AgentId,
    ) {
        self.agent_orchestrator
            .observe_agent_transcript(request_id, agent_id);
        self.notify_runtime_event();
    }

    /// 撤销 overview observation；mismatch 静默丢弃，不产生事件，无需唤醒。
    pub(super) fn stop_observing_agents(
        &mut self,
        observation_id: runtime_domain::agent::AgentObservationId,
        generation: runtime_domain::agent::AgentRuntimeGeneration,
    ) {
        self.agent_orchestrator
            .stop_observation(observation_id, generation);
    }

    /// 撤销 per-agent observation；mismatch 静默丢弃。
    pub(super) fn stop_observing_agent_transcript(
        &mut self,
        observation_id: runtime_domain::agent::AgentObservationId,
        generation: runtime_domain::agent::AgentRuntimeGeneration,
    ) {
        self.agent_orchestrator
            .stop_observation(observation_id, generation);
    }

    /// typed child permission response；closed rejection 映射为固定安全文案。
    pub(super) fn respond_child_agent_permission(
        &mut self,
        target: runtime_domain::agent::AgentPermissionTarget,
        option_id: Option<String>,
    ) -> Result<(), String> {
        self.agent_orchestrator
            .respond_agent_permission(target, option_id)
            .map_err(|rejection| rejection.closed_message().to_string())
    }

    /// typed subtree stop；closed rejection 映射为固定安全文案。
    pub(super) fn stop_child_agent_with_generation(
        &mut self,
        agent_id: runtime_domain::agent::AgentId,
        generation: runtime_domain::agent::AgentRuntimeGeneration,
    ) -> Result<(), String> {
        self.agent_orchestrator
            .stop_agent(agent_id, generation)
            .map_err(|rejection| rejection.closed_message().to_string())
    }

    /// 取出 orchestrator queue 的 projection facts，供 runtime event consumer 边界 flush。
    pub(super) fn drain_agent_projection_events(
        &mut self,
    ) -> Vec<runtime_domain::agent::AgentProjectionEvent> {
        self.agent_orchestrator.drain_projection_events()
    }

    pub(super) fn dispose_child_agents_for_session_transition(&mut self) -> Result<(), String> {
        self.agent_orchestrator
            .dispose_children_for_session_transition()
            .map_err(|_| "Child Agent cleanup is pending".to_string())
    }

    pub(super) fn agent_activity(&self) -> super::agent::AgentRuntimeActivity {
        self.agent_orchestrator.activity()
    }

    #[cfg(test)]
    pub(super) fn agent_orchestrator_has_pending_work(&self) -> bool {
        self.agent_orchestrator.has_pending_work()
    }

    #[cfg(test)]
    pub(super) fn agent_port(&self) -> &dyn AgentRuntimePort {
        self.agent_orchestrator.main_port()
    }

    #[cfg(test)]
    pub(super) fn agent_port_mut(&mut self) -> &mut dyn AgentRuntimePort {
        self.agent_orchestrator.main_port_mut()
    }

    pub(super) fn agent_session(&self) -> Result<&dyn AgentSessionCapability, String> {
        self.agent_orchestrator
            .main_session()
            .ok_or_else(|| "Agent adapter does not provide session capability".to_string())
    }

    pub(super) fn agent_session_mut(&mut self) -> Result<&mut dyn AgentSessionCapability, String> {
        self.agent_orchestrator
            .main_session_mut()
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
        self.agent_orchestrator
            .main_session_mut()
            .expect("runtime test requires an Agent session capability")
            .test_harness()
            .expect("runtime test requires an Agent fixture harness")
    }

    #[cfg(test)]
    pub(super) fn agent_test_harness_ref(&self) -> &dyn super::agent::AgentRuntimeTestHarness {
        self.agent_orchestrator
            .main_session()
            .expect("runtime test requires an Agent session capability")
            .test_harness_ref()
            .expect("runtime test requires an Agent fixture harness")
    }

    #[cfg(test)]
    pub(super) fn agent_root_context_for_test(
        &self,
    ) -> Option<crate::runtime::agent_capability_context::AgentCapabilityContext> {
        self.agent_orchestrator.root_context()
    }

    #[cfg(test)]
    pub(super) fn register_child_agent_for_test(
        &mut self,
        agent_id: runtime_domain::agent::AgentId,
        parent_agent_id: runtime_domain::agent::AgentId,
        turn_id: runtime_domain::agent::AgentTurnId,
        title: runtime_domain::agent::AgentTitle,
        context: crate::runtime::agent_capability_context::AgentCapabilityContext,
        runtime: Box<dyn AgentRuntimePort>,
    ) {
        self.agent_orchestrator.register_child_for_test(
            agent_id,
            parent_agent_id,
            turn_id,
            title,
            context,
            runtime,
        );
    }

    #[cfg(test)]
    pub(super) fn child_agent_count_for_test(&self) -> usize {
        self.agent_orchestrator.child_count()
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
        let (spawn_agents_tool, spawn_agents_receiver) =
            SpawnAgentsTool::channel(runtime_event_notifier.clone());
        let extension_hooks = ExtensionHookRegistry::new();
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
            Some(spawn_agents_tool.clone()),
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
        let session_workspace_tools =
            session_tools_for_manager(&tool_catalog, prompt_assembly_snapshot.manager.as_ref());
        // Consumer 在 activation callback 中从 typed Context 换入 live generation；
        // bootstrap placeholder 不得提前持有 provider 的 raw notifier。
        let session_store_worker = SessionStoreWorker::new(RuntimeEventNotifier::default());
        let context_budget_worker = ContextBudgetWorker::new(RuntimeEventNotifier::default())
            .map_err(|error| error.to_string())?;
        let child_factory = committed_agent_child_factory(&plugins);
        let agent_runtime = construct_committed_agent_runtime(
            &plugins,
            AgentRuntimeGrantSource::new(
                options,
                &extension_hooks,
                &session_workspace_tools,
                &tool_catalog,
                &prompt_assembly,
                session_backend_views.as_ref().map(|views| &views.port),
                &llm_port,
                &permission_policy,
                TERMINAL_APPROVAL_PROVIDER_ID,
            ),
        )?;
        let model_refresh = ModelRefreshWorker::new(RuntimeEventNotifier::default());
        let has_session_backend = session_backend_views.is_some();
        let agent_child_static_grants = agent_child_static_grants(
            child_factory.as_ref(),
            options,
            TERMINAL_APPROVAL_PROVIDER_ID,
        );
        let mut agent_orchestrator =
            AgentOrchestrator::new(agent_runtime, child_factory, agent_child_static_grants);
        agent_orchestrator.bind_session_port(
            session_backend_views
                .as_ref()
                .map(|views| Arc::clone(&views.port)),
        );
        let mut components = Self {
            agent_orchestrator,
            model_refresh,
            extension_hooks,
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
            spawn_agents_receiver,
            spawn_agents_tool,
            activation_staging: ComponentActivationStaging {
                approval_registration: Some(approval_registration),
                provider_registrations: Some(provider_registrations),
                tool_registration: Some(tool_registration),
                prompt_registration: Some(prompt_registration),
                session_backend_registration,
                runtime_wake: None,
                extension_bundle: None,
                extension_source: None,
                is_session_backend_replacement: false,
            },
            plugin_loader,
            plugins,
            prepared_plugin_commit: None,
            is_agent_replacement_activating: false,
            #[cfg(test)]
            plugin_transaction_trace: None,
            #[cfg(test)]
            agent_grant_materialization_probe: None,
            lifecycle: ComponentLifecycleExecutor::default(),
            finalization: RuntimeFinalization::Open,
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

    /// 将已完成 handshake 的 extension bundle 接入 lifecycle。
    ///
    /// discovery/stdio 启动在 async 边界外完成；此处只接收 opaque set，并让 graph 决定
    /// `tool_catalog` 缺失时的 Pending 状态。默认 composition 不包含该 component。
    pub(super) fn mount_extension_bundle(&mut self, bundle: ExtensionBundle) -> Result<(), String> {
        if self.finalization != RuntimeFinalization::Open {
            return Err("Runtime components are shut down".to_string());
        }
        let source = bundle
            .rediscovery_source()
            .ok_or_else(|| "external extension discovery source is not available".to_string())?;
        if self
            .plugins
            .implementation(EXTERNAL_EXTENSION_COMPONENT)
            .is_some()
        {
            return self.replace_extension_bundle(bundle);
        }
        self.activation_staging.extension_bundle = Some(bundle);
        self.activation_staging.extension_source = Some(source);
        let desired = match self.desired_with_external_extension(true) {
            Ok(desired) => desired,
            Err(error) => {
                self.activation_staging.extension_bundle.take();
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
                self.activation_staging.extension_bundle.take();
                self.activation_staging.extension_source.take();
                return Err(error.to_string());
            }
        };
        let result = self.commit_prepared_plugin_reconciliation(
            None,
            desired,
            reconciliation,
            PreparedAgentRuntimeCommit::Keep,
            ComponentLifecycleMode::Reconfigure,
        );
        if result.is_err() {
            self.activation_staging.extension_bundle.take();
            self.activation_staging.extension_source.take();
        }
        result
    }

    /// 先撤销旧 extension component，再接入 fresh set；两个 generation 不会并存。
    pub(super) fn replace_extension_bundle(
        &mut self,
        bundle: ExtensionBundle,
    ) -> Result<(), String> {
        self.remove_extension_bundle()?;
        self.mount_extension_bundle(bundle)
    }

    /// 从 desired composition 移除 extension component，并执行其 effect inverse。
    pub(super) fn remove_extension_bundle(&mut self) -> Result<(), String> {
        self.activation_staging.extension_bundle.take();
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
            None,
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
        if self.finalization != RuntimeFinalization::Open {
            return Err("Runtime components are shut down".to_string());
        }
        let agent_action = self.classify_agent_plugin_reconciliation(&desired)?;
        let reconciliation = self
            .plugin_loader
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        let agent_runtime = Self::prepare_agent_runtime_commit(agent_action);
        self.commit_prepared_plugin_reconciliation(
            Some(options),
            desired,
            reconciliation,
            agent_runtime,
            mode,
        )
    }

    #[cfg(test)]
    fn reconcile_plugin_composition_with_catalog(
        &mut self,
        options: &AppRuntimeOptions,
        catalog: &PluginFactoryCatalog<RuntimePluginImplementation>,
        desired: DesiredPluginComposition,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        let agent_action = self.classify_agent_plugin_reconciliation(&desired)?;
        let reconciliation = catalog
            .prepare_reconciliation(&desired, &self.plugins)
            .map_err(|error| error.to_string())?;
        let agent_runtime = Self::prepare_agent_runtime_commit(agent_action);
        self.commit_prepared_plugin_reconciliation(
            Some(options),
            desired,
            reconciliation,
            agent_runtime,
            mode,
        )
    }

    fn classify_agent_plugin_reconciliation(
        &self,
        desired: &DesiredPluginComposition,
    ) -> Result<AgentPluginReconciliation, String> {
        let desired_type = desired
            .iter()
            .find(|(component_id, _)| component_id.as_str() == AGENT_RUNTIME_COMPONENT)
            .map(|(_, plugin_type)| plugin_type);
        let observed = self.plugins.observed();
        let observed_type = observed
            .iter()
            .find(|(component_id, _)| component_id.as_str() == AGENT_RUNTIME_COMPONENT)
            .map(|(_, plugin_type)| plugin_type);
        classify_agent_plugin_reconciliation(observed_type, desired_type)
    }

    fn prepare_agent_runtime_commit(
        action: AgentPluginReconciliation,
    ) -> PreparedAgentRuntimeCommit {
        match action {
            AgentPluginReconciliation::Keep => PreparedAgentRuntimeCommit::Keep,
            AgentPluginReconciliation::Replace => PreparedAgentRuntimeCommit::ReplacePending,
        }
    }

    fn commit_prepared_plugin_reconciliation(
        &mut self,
        agent_options: Option<&AppRuntimeOptions>,
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
            let mut callbacks = PluginCommitCallbacks {
                components,
                agent_options,
            };
            lifecycle.reconcile_definitions_with_commit(definitions, &mut callbacks, mode)
        })
        .map_err(|error| format!("{error:?}"))
    }

    pub(super) fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String> {
        if self.finalization != RuntimeFinalization::Open {
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
        if self.finalization != RuntimeFinalization::Open {
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
        self.finalize_agent_runtime_replacement()?;

        let fresh_llm_port = LlmPort::new();
        let fresh_provider_registrations = fresh_llm_port
            .mount_builtin_providers("models-config", &options.loaded_models.provider_configs)
            .map_err(|error| error.to_string())?;
        let (fresh_tool_catalog, fresh_tool_registration) = conversation_workspace_tool_catalog(
            &options.managed_ripgrep,
            &options.hunea_config_dir,
            Some(self.spawn_agents_tool.clone()),
        )
        .map_err(|error| error.to_string())?;
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
            AgentRuntimeGrantSource::new(
                options,
                &self.extension_hooks,
                &session_workspace_tools,
                &fresh_tool_catalog,
                &fresh_prompt_assembly,
                self.session_backend_views.as_ref().map(|views| &views.port),
                &fresh_llm_port,
                &self.permission_policy,
                &self.permission_provider_id,
            ),
        )?;
        let fresh_child_factory = committed_agent_child_factory(&self.plugins);
        let fresh_child_static_grants = agent_child_static_grants(
            fresh_child_factory.as_ref(),
            options,
            &self.permission_provider_id,
        );
        self.agent_orchestrator
            .replace_main(
                fresh_agent_runtime,
                fresh_child_factory,
                fresh_child_static_grants,
            )
            .map_err(|error| error.to_string())?;
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
        if self.finalization != RuntimeFinalization::Open {
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
            self.with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [AGENT_RUNTIME_COMPONENT, SESSION_PERSISTENCE.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .map_err(|retry_error| format!("{cleanup_error}; {retry_error}"))?;
            self.restore_ephemeral_session_consumers(options)
                .map_err(|fallback_error| format!("{cleanup_error}; {fallback_error}"))?;
            return Err(cleanup_error);
        }
        self.finalize_agent_runtime_replacement()?;

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
        let fresh_agent_runtime = match self.fresh_agent_runtime(options, Some(&fresh_views.port)) {
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
        let fresh_child_factory = committed_agent_child_factory(&self.plugins);
        let fresh_child_static_grants = agent_child_static_grants(
            fresh_child_factory.as_ref(),
            options,
            &self.permission_provider_id,
        );
        self.agent_orchestrator
            .replace_main(
                fresh_agent_runtime,
                fresh_child_factory,
                fresh_child_static_grants,
            )
            .map_err(|error| error.to_string())?;
        self.session_port = fresh_session_port;
        self.session_backend_views = Some(fresh_views);
        self.agent_orchestrator.bind_session_port(
            self.session_backend_views
                .as_ref()
                .map(|views| Arc::clone(&views.port)),
        );
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
        session_port: Option<&Arc<dyn session_store::SessionPort>>,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        construct_committed_agent_runtime(
            &self.plugins,
            self.agent_runtime_grant_source(options, session_port),
        )
    }

    fn agent_runtime_grant_source<'a>(
        &'a self,
        options: &'a AppRuntimeOptions,
        session_port: Option<&'a Arc<dyn session_store::SessionPort>>,
    ) -> AgentRuntimeGrantSource<'a> {
        self.agent_runtime_grant_source_with_hooks(options, &self.extension_hooks, session_port)
    }

    fn agent_runtime_grant_source_with_hooks<'a>(
        &'a self,
        options: &'a AppRuntimeOptions,
        extension_hooks: &'a ExtensionHookRegistry,
        session_port: Option<&'a Arc<dyn session_store::SessionPort>>,
    ) -> AgentRuntimeGrantSource<'a> {
        let source = AgentRuntimeGrantSource::new(
            options,
            extension_hooks,
            &self.session_workspace_tools,
            &self.tool_catalog,
            &self.prompt_assembly,
            session_port,
            &self.llm_port,
            &self.permission_policy,
            &self.permission_provider_id,
        );
        #[cfg(test)]
        let source = source.with_materialization_probe(
            self.agent_grant_materialization_probe
                .as_ref()
                .map(Arc::clone),
        );
        source
    }

    fn restore_ephemeral_session_consumers(
        &mut self,
        options: &AppRuntimeOptions,
    ) -> Result<(), String> {
        self.finalize_agent_runtime_replacement()?;
        let fresh_agent_runtime = self
            .fresh_agent_runtime(options, None)
            .map_err(|error| format!("restore ephemeral Agent after backend failure: {error}"))?;
        let fresh_child_factory = committed_agent_child_factory(&self.plugins);
        let fresh_child_static_grants = agent_child_static_grants(
            fresh_child_factory.as_ref(),
            options,
            &self.permission_provider_id,
        );
        self.agent_orchestrator
            .replace_main(
                fresh_agent_runtime,
                fresh_child_factory,
                fresh_child_static_grants,
            )
            .map_err(|error| error.to_string())?;
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
        if self.finalization != RuntimeFinalization::Open {
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
        self.finalize_agent_runtime_replacement()?;

        let provider_id = provider_id.into();
        let fresh_policy = PermissionPolicy::new();
        let fresh_registration = fresh_policy
            .register(owner, provider_id.clone(), factory)
            .map_err(|error| error.to_string())?;
        let fresh_agent_runtime = match construct_committed_agent_runtime(
            &self.plugins,
            AgentRuntimeGrantSource::new(
                options,
                &self.extension_hooks,
                &self.session_workspace_tools,
                &self.tool_catalog,
                &self.prompt_assembly,
                self.session_backend_views.as_ref().map(|views| &views.port),
                &self.llm_port,
                &fresh_policy,
                &provider_id,
            ),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                fresh_policy.deactivate();
                drop(fresh_registration);
                return Err(error);
            }
        };

        let fresh_child_factory = committed_agent_child_factory(&self.plugins);
        let fresh_child_static_grants =
            agent_child_static_grants(fresh_child_factory.as_ref(), options, &provider_id);
        self.permission_policy = fresh_policy;
        self.permission_provider_id = provider_id;
        self.agent_orchestrator
            .replace_main(
                fresh_agent_runtime,
                fresh_child_factory,
                fresh_child_static_grants,
            )
            .map_err(|error| error.to_string())?;
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
        let hooks = context
            .require::<ExtensionHookRegistryCapability>()
            .map_err(|error| error.to_string())?;
        let bundle = match self.activation_staging.extension_bundle.take() {
            Some(bundle) => bundle,
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
            .or_else(|| bundle.rediscovery_source())
            .ok_or_else(|| "external extension discovery source is not available".to_string())?;
        let mount = bundle
            .mount(&catalog, &hooks, EXTERNAL_EXTENSION_COMPONENT)
            .map_err(|error| error.to_string())?;
        let mount = Arc::new(Mutex::new(mount));
        self.extension_source = Some(Arc::clone(&source));
        let disposer = Arc::clone(&mount);
        scope
            .register("extension_mount", move || {
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

    fn activate_extension_hooks(
        &mut self,
        _scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        context
            .publish::<ExtensionHookRegistryCapability>(self.extension_hooks.clone())
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
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        _mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        let (requires_event_stream, requires_extension_hooks) = self
            .plugins
            .instance(AGENT_RUNTIME_COMPONENT)
            .map(|instance| {
                (
                    instance
                        .descriptor()
                        .declares_required(RUNTIME_EVENT_STREAM.capability),
                    instance
                        .descriptor()
                        .declares_required(EXTENSION_HOOKS.capability),
                )
            })
            .ok_or_else(|| "Agent component has no committed plugin implementation".to_string())?;
        let event_stream = if requires_event_stream {
            Some(
                context
                    .require::<RuntimeEventStreamCapability>()
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let extension_hooks = if requires_extension_hooks {
            Some(
                context
                    .require::<ExtensionHookRegistryCapability>()
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let mut grants = AgentRuntimeActivationGrants::empty();
        if let Some(lease) = event_stream.clone() {
            grants = grants.with_event_stream(lease);
        }
        if let Some(lease) = extension_hooks.clone() {
            grants = grants.with_extension_hooks(lease);
        }

        let (root_context, child_leases, main_tools) =
            if self.agent_orchestrator.has_child_factory() {
                let tool_lease = context
                    .require::<ToolCatalogCapability>()
                    .map_err(|error| error.to_string())?;
                let prompt_lease = context
                    .require::<PromptAssemblyCapability>()
                    .map_err(|error| error.to_string())?;
                let llm_lease = context
                    .require::<LlmPortCapability>()
                    .map_err(|error| error.to_string())?;
                let permission_lease = context
                    .require::<PermissionPolicyCapability>()
                    .map_err(|error| error.to_string())?;
                let root_context = self.agent_orchestrator.build_root_context(
                    scope,
                    &tool_lease,
                    &prompt_lease,
                    self.session_workspace_tools
                        .definitions()
                        .definitions()
                        .map(|definition| definition.name.clone())
                        .collect::<Vec<_>>(),
                )?;
                let child_leases = AgentChildRuntimeLeases::new(
                    event_stream.ok_or_else(|| {
                        "Agent child capability requires runtime event stream".to_string()
                    })?,
                    extension_hooks.ok_or_else(|| {
                        "Agent child capability requires extension hooks".to_string()
                    })?,
                    llm_lease,
                    permission_lease,
                );
                let main_tools = root_context.tools().map_err(|error| error.to_string())?;
                (Some(root_context), Some(child_leases), Some(main_tools))
            } else {
                (None, None, None)
            };
        if let Some(main_tools) = main_tools {
            grants = grants.with_tools(main_tools);
        }
        self.agent_orchestrator
            .activate_main(grants, root_context, child_leases)?;
        self.agent_orchestrator.bind_session_port(
            self.session_backend_views
                .as_ref()
                .map(|views| Arc::clone(&views.port)),
        );
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
        let result = match mode {
            ComponentLifecycleMode::Shutdown => self
                .agent_orchestrator
                .shutdown()
                .map_err(|_| "Agent adapter failed to shut down".to_string()),
            _ => self
                .agent_orchestrator
                .suspend()
                .map_err(|_| "Agent adapter failed to suspend".to_string()),
        };
        self.agent_orchestrator.bind_session_port(None);
        result
    }

    fn finalize_agent_runtime_replacement(&mut self) -> Result<(), String> {
        let result = self
            .agent_orchestrator
            .shutdown()
            .map_err(|_| "Agent adapter failed to finalize for replacement".to_string());
        self.agent_orchestrator.bind_session_port(None);
        result
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

    fn quiesce_extension_hooks(&mut self, mode: ComponentLifecycleMode) -> Result<(), String> {
        if mode != ComponentLifecycleMode::Reconfigure {
            return Ok(());
        }
        self.extension_hooks = ExtensionHookRegistry::new();
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
        if self.finalization == RuntimeFinalization::Succeeded {
            return Ok(());
        }
        self.finalization = RuntimeFinalization::Finalizing;
        let lifecycle_error = self
            .with_lifecycle(|lifecycle, components| lifecycle.shutdown(components))
            .err()
            .map(|error| format!("{error:?}"));
        let agent_error = lifecycle_error.is_none().then(|| {
            self.agent_orchestrator
                .shutdown()
                .err()
                .map(|_| "Agent runtime finalization failed".to_string())
        });
        let agent_error = agent_error.flatten();
        self.is_agent_replacement_activating = false;
        match (lifecycle_error, agent_error) {
            (None, None) => {
                self.finalization = RuntimeFinalization::Succeeded;
                Ok(())
            }
            (Some(error), None) | (None, Some(error)) => Err(error),
            (Some(lifecycle), Some(agent)) => Err(format!("{lifecycle}; {agent}")),
        }
    }

    fn prepare_plugin_authority(
        &mut self,
        agent_options: Option<&AppRuntimeOptions>,
    ) -> Result<(), String> {
        let prepared = self
            .prepared_plugin_commit
            .as_ref()
            .ok_or_else(|| "plugin authority preparation is missing".to_string())?;
        let is_agent_replacement = matches!(
            &prepared.agent_runtime,
            PreparedAgentRuntimeCommit::ReplacePending
                | PreparedAgentRuntimeCommit::ReplaceReady(_)
        );
        if is_agent_replacement {
            if matches!(
                &prepared.agent_runtime,
                PreparedAgentRuntimeCommit::ReplaceReady(_)
            ) {
                return Err("Agent adapter candidate is already prepared".to_string());
            }
            self.finalize_agent_runtime_replacement()?;
            let next_generation = self
                .agent_orchestrator
                .prepare_main_replacement_generation()
                .map_err(|error| error.to_string())?;

            let (candidate, child_factory, child_static_grants) = {
                let options = agent_options.ok_or_else(|| {
                    "Agent replacement construction options are missing".to_string()
                })?;
                let prepared = self
                    .prepared_plugin_commit
                    .as_ref()
                    .expect("prepared plugin commit must survive Agent finalization");
                let instance = prepared
                    .reconciliation
                    .prospective_instance(&self.plugins, AGENT_RUNTIME_COMPONENT)
                    .ok_or_else(|| {
                        "Agent replacement has no prospective plugin instance".to_string()
                    })?;
                let child_factory = instance.implementation().child_factory();
                let child_static_grants = child_factory.as_ref().map(|_| {
                    AgentChildRuntimeStaticGrants::new(
                        options.runtime_request_policy.clone(),
                        options.loaded_models.clone(),
                        Arc::clone(&options.dynamic_environment_observer),
                        options.hunea_config_dir.clone(),
                        self.permission_provider_id.clone(),
                    )
                });
                let grant_source = self.agent_runtime_grant_source(
                    options,
                    self.session_backend_views.as_ref().map(|views| &views.port),
                );
                (
                    construct_agent_runtime(instance, grant_source)?,
                    child_factory,
                    child_static_grants,
                )
            };
            let prepared = self
                .prepared_plugin_commit
                .as_mut()
                .expect("prepared plugin commit must survive Agent construction");
            if !matches!(
                prepared.agent_runtime,
                PreparedAgentRuntimeCommit::ReplacePending
            ) {
                unreachable!("Agent replacement kind must remain stable during construction")
            }
            prepared.agent_runtime = PreparedAgentRuntimeCommit::ReplaceReady(Box::new(
                PreparedAgentRuntimeReplacement {
                    candidate,
                    child_factory,
                    child_static_grants,
                    next_generation,
                },
            ));
        }
        #[cfg(test)]
        self.record_plugin_transaction_event("prepare:authority");
        Ok(())
    }

    fn commit_plugin_authority(&mut self) {
        let prepared = self
            .prepared_plugin_commit
            .take()
            .expect("plugin authority commit must have a prepared composition");
        let replacement = match prepared.agent_runtime {
            PreparedAgentRuntimeCommit::Keep => None,
            PreparedAgentRuntimeCommit::ReplaceReady(replacement) => Some(*replacement),
            PreparedAgentRuntimeCommit::ReplacePending => {
                unreachable!("Agent replacement must be prepared before authority commit")
            }
        };
        if let Some(replacement) = replacement {
            self.agent_orchestrator.commit_prepared_main_replacement(
                replacement.candidate,
                replacement.child_factory,
                replacement.child_static_grants,
                replacement.next_generation,
            );
            self.is_agent_replacement_activating = true;
        }
        self.plugins.commit_reconciliation(prepared.reconciliation);
        self.plugin_loader.commit_desired(prepared.desired);
        #[cfg(test)]
        self.record_plugin_transaction_event("authority:commit");
    }

    fn abort_plugin_authority(&mut self) {
        self.prepared_plugin_commit.take();
        #[cfg(test)]
        self.record_plugin_transaction_event("authority:abort");
    }
}

impl ComponentLifecycleCallbacks for PluginCommitCallbacks<'_> {
    fn prepare_authority(&mut self) -> Result<(), String> {
        self.components.prepare_plugin_authority(self.agent_options)
    }

    fn commit_authority(&mut self) {
        self.components.commit_plugin_authority();
    }

    fn abort_authority(&mut self) {
        self.components.abort_plugin_authority();
    }

    fn activate_component(
        &mut self,
        component_id: &str,
        scope: &EffectScope,
        context: &mut ComponentActivationContext<'_>,
        mode: ComponentLifecycleMode,
    ) -> Result<ComponentActivationOutcome, String> {
        ComponentLifecycleCallbacks::activate_component(
            self.components,
            component_id,
            scope,
            context,
            mode,
        )
    }

    fn quiesce_component(
        &mut self,
        component_id: &str,
        mode: ComponentLifecycleMode,
    ) -> Result<(), String> {
        ComponentLifecycleCallbacks::quiesce_component(self.components, component_id, mode)
    }
}

impl ComponentLifecycleCallbacks for RuntimeComponents {
    fn prepare_authority(&mut self) -> Result<(), String> {
        self.prepare_plugin_authority(None)
    }

    fn commit_authority(&mut self) {
        self.commit_plugin_authority();
    }

    fn abort_authority(&mut self) {
        self.abort_plugin_authority();
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
        num::NonZeroUsize,
        pin::Pin,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use super::*;
    use crate::runtime::agent::{AgentRuntimeActivity, AgentSessionRestore};
    use crate::runtime::agent_capability_context::{AgentChildCapabilityGrants, AgentContextOwner};
    use crate::runtime::lifecycle::{ComponentDefinition, ComponentState};
    use agent_kernel_protocol::{
        AgentKernelCapability, AgentKernelCommand, AgentKernelCommandParams,
        AgentKernelCommandReceipt, AgentKernelCommandResult, AgentKernelEvent,
        AgentKernelEventKind, AgentKernelEventNotification, AgentKernelInitializeParams,
        AgentKernelInitializeResult, AgentKernelMethod, AgentKernelRequest, AgentKernelResponse,
        AgentKernelShutdownResult,
    };
    use agent_kernel_runtime::{
        AgentKernelConnectError, AgentKernelConnection, AgentKernelEventSink,
        AgentKernelEventStream, AgentKernelRequestTransport, AgentKernelTransportError,
    };
    use extension_hook_runtime::{
        BeforeTurnDecision, BeforeTurnPayload, HookId, HookOwnerId, HookPriority,
        HookRegistrationOptions,
    };
    use extension_protocol::{
        BeforeTurnHookParams, BeforeTurnHookResult, ExtensionCapability, ExtensionMethod,
        ExtensionRequest, ExtensionResponse, HookCancelResult, HookDescriptor, HookPhase,
        HooksListResult, InitializeResult, ToolDescriptor, ToolsListResult,
    };
    use extension_runtime::{
        ExtensionBundleSource, ExtensionClient, ExtensionDiscoveryError, ExtensionOptions,
        ExtensionRequestFuture, ExtensionRequestTransport, ExtensionTransportError,
    };
    use runtime_domain::agent::{
        AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentGroupCompletion,
        AgentId, AgentOutcome, AgentProjectionEvent, AgentRuntime, AgentRuntimeError, AgentTurnId,
        AgentTurnRequest,
    };
    use runtime_domain::prompt_assembly::{
        PromptPreludeSection, PromptSourceKind, PromptSourceOrigin,
    };
    use runtime_domain::session::ConversationTurnRequest;
    use session_store::SessionLifecycleStore;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::{
        ToolCall, ToolExecutionContext, ToolExecutor, ToolInvocationIdentity, ToolResultOutcome,
    };

    #[derive(Default)]
    struct ComponentKernelSource {
        connect_count: AtomicUsize,
        generations: Mutex<Vec<Arc<ComponentKernelGeneration>>>,
    }

    struct ComponentKernelGeneration {
        event_sender: Mutex<Option<AgentKernelEventSink>>,
        commands: Mutex<Vec<AgentKernelCommandParams>>,
        shutdowns: AtomicUsize,
    }

    struct ComponentKernelTransport {
        generation: Arc<ComponentKernelGeneration>,
    }

    impl ComponentKernelSource {
        fn generation(&self, index: usize) -> Arc<ComponentKernelGeneration> {
            Arc::clone(
                self.generations
                    .lock()
                    .expect("kernel generations")
                    .get(index)
                    .expect("kernel generation should exist"),
            )
        }
    }

    impl AgentKernelSource for ComponentKernelSource {
        fn connect(&self) -> Result<AgentKernelConnection, AgentKernelConnectError> {
            let (event_sender, event_receiver) = AgentKernelEventStream::bounded(
                NonZeroUsize::new(16).expect("literal event capacity is non-zero"),
            );
            let generation = Arc::new(ComponentKernelGeneration {
                event_sender: Mutex::new(Some(event_sender)),
                commands: Mutex::new(Vec::new()),
                shutdowns: AtomicUsize::new(0),
            });
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            self.generations
                .lock()
                .expect("kernel generations")
                .push(Arc::clone(&generation));
            Ok(AgentKernelConnection::new(
                ComponentKernelTransport { generation },
                event_receiver,
            ))
        }
    }

    impl AgentKernelRequestTransport for ComponentKernelTransport {
        fn request(
            &self,
            request: AgentKernelRequest,
        ) -> Result<AgentKernelResponse, AgentKernelTransportError> {
            if self
                .generation
                .event_sender
                .lock()
                .expect("kernel event sender")
                .is_none()
            {
                return Err(AgentKernelTransportError::ShutDown);
            }
            let request_id = request.request_id().to_string();
            match request.method() {
                AgentKernelMethod::Initialize => {
                    request
                        .decode_params::<AgentKernelInitializeParams>()
                        .map_err(|_| AgentKernelTransportError::Protocol)?;
                    AgentKernelResponse::success(
                        request_id,
                        AgentKernelInitializeResult {
                            protocol: agent_kernel_protocol::PROTOCOL_NAME.to_string(),
                            version: agent_kernel_protocol::PROTOCOL_VERSION,
                            capabilities: vec![
                                AgentKernelCapability::Events,
                                AgentKernelCapability::Interrupt,
                                AgentKernelCapability::PermissionResponse,
                                AgentKernelCapability::StructuredErrors,
                            ],
                            accepted_host_capabilities: Vec::new(),
                        },
                    )
                    .map_err(|_| AgentKernelTransportError::Protocol)
                }
                AgentKernelMethod::AgentCommand => {
                    let params = request
                        .decode_params::<AgentKernelCommandParams>()
                        .map_err(|_| AgentKernelTransportError::Protocol)?;
                    self.generation
                        .commands
                        .lock()
                        .expect("kernel commands")
                        .push(params.clone());
                    let receipt = match &params.command {
                        AgentKernelCommand::SubmitTurn {
                            turn_id, request, ..
                        } => AgentKernelCommandReceipt::TurnStarted {
                            turn_id: *turn_id,
                            target: request.target.clone(),
                            activity_label: request.target.model_id.clone(),
                        },
                        AgentKernelCommand::Interrupt { target, .. } => {
                            AgentKernelCommandReceipt::Interrupted {
                                target: target.clone(),
                            }
                        }
                        AgentKernelCommand::RespondPermission { .. } => {
                            AgentKernelCommandReceipt::Accepted
                        }
                    };
                    AgentKernelResponse::success(
                        request_id,
                        AgentKernelCommandResult {
                            command_id: params.command_id,
                            receipt,
                        },
                    )
                    .map_err(|_| AgentKernelTransportError::Protocol)
                }
                AgentKernelMethod::Shutdown => AgentKernelResponse::success(
                    request_id,
                    AgentKernelShutdownResult { drained: true },
                )
                .map_err(|_| AgentKernelTransportError::Protocol),
            }
        }

        fn shutdown(&self) -> Result<(), AgentKernelTransportError> {
            if self
                .generation
                .event_sender
                .lock()
                .expect("kernel event sender")
                .take()
                .is_some()
            {
                self.generation.shutdowns.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    impl ComponentKernelGeneration {
        fn send_delta(&self, sequence: u64, command_index: usize, content: &str) -> bool {
            let command = self
                .commands
                .lock()
                .expect("kernel commands")
                .get(command_index)
                .cloned()
                .expect("kernel command should exist");
            let AgentKernelCommand::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } = command.command
            else {
                panic!("expected submit command")
            };
            self.event_sender
                .lock()
                .expect("kernel event sender")
                .as_ref()
                .is_some_and(|sender| {
                    sender
                        .send(AgentKernelEventNotification::new(
                            sequence,
                            command.command_id,
                            AgentKernelEvent {
                                agent_id,
                                turn_id,
                                target: request.target,
                                kind: AgentKernelEventKind::AssistantDelta {
                                    content: content.to_string(),
                                },
                            },
                        ))
                        .is_ok()
                })
        }
    }

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
                            ExtensionCapability::Hooks,
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
                ExtensionMethod::HooksList => ExtensionResponse::success(
                    request.request_id().to_string(),
                    HooksListResult {
                        hooks: vec![HookDescriptor {
                            hook_id: "external-hook".to_string(),
                            phase: HookPhase::BeforeTurn,
                            priority: 0,
                        }],
                    },
                ),
                ExtensionMethod::HooksBeforeTurn => {
                    let params = request
                        .decode_params::<BeforeTurnHookParams>()
                        .expect("hook params should decode");
                    ExtensionResponse::success(
                        request.request_id().to_string(),
                        BeforeTurnHookResult::Continue {
                            items: params.items,
                        },
                    )
                }
                ExtensionMethod::HooksCancel => ExtensionResponse::success(
                    request.request_id().to_string(),
                    HookCancelResult { accepted: true },
                ),
                _ => unreachable!("static extension only serves discovery and before_turn"),
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

    impl ExtensionBundleSource for StaticExtensionSource {
        fn discover(&self) -> Result<extension_runtime::ExtensionBundle, ExtensionDiscoveryError> {
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
                    ExtensionClient::new(transport, ExtensionOptions::default()).discover(),
                )?;
            Ok(set.with_rediscovery_source(Arc::new(self.clone())))
        }
    }

    fn discovered_extension_bundle(
        transport: StaticExtensionTransport,
    ) -> extension_runtime::ExtensionBundle {
        let source = Arc::new(StaticExtensionSource {
            shutdowns: Arc::clone(&transport.shutdowns),
            discoveries: Arc::new(AtomicUsize::new(0)),
        });
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(ExtensionClient::new(transport, ExtensionOptions::default()).discover())
            .expect("static extension discovery should succeed")
            .with_rediscovery_source(source)
    }

    struct BlockingHookState {
        hook_waiter: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        hook_started: Mutex<Option<mpsc::Sender<String>>>,
        cancel_targets: Mutex<Vec<String>>,
        shutdowns: AtomicUsize,
    }

    #[derive(Clone)]
    struct BlockingHookTransport {
        state: Arc<BlockingHookState>,
    }

    impl ExtensionRequestTransport for BlockingHookTransport {
        fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_> {
            let request_id = request.request_id().to_string();
            match request.method() {
                ExtensionMethod::Initialize => {
                    let response = ExtensionResponse::success(
                        request_id,
                        InitializeResult {
                            protocol: extension_protocol::PROTOCOL_NAME.to_string(),
                            version: extension_protocol::PROTOCOL_VERSION,
                            capabilities: vec![
                                ExtensionCapability::Cancel,
                                ExtensionCapability::StructuredErrors,
                                ExtensionCapability::Hooks,
                            ],
                        },
                    )
                    .expect("response should encode");
                    Box::pin(async move { Ok(response) })
                }
                ExtensionMethod::ToolsList => {
                    let response = ExtensionResponse::success(
                        request_id,
                        ToolsListResult { tools: Vec::new() },
                    )
                    .expect("response should encode");
                    Box::pin(async move { Ok(response) })
                }
                ExtensionMethod::HooksList => {
                    let response = ExtensionResponse::success(
                        request_id,
                        HooksListResult {
                            hooks: vec![HookDescriptor {
                                hook_id: "external-hook".to_string(),
                                phase: HookPhase::BeforeTurn,
                                priority: 0,
                            }],
                        },
                    )
                    .expect("response should encode");
                    Box::pin(async move { Ok(response) })
                }
                ExtensionMethod::HooksBeforeTurn => {
                    let params = request
                        .decode_params::<BeforeTurnHookParams>()
                        .expect("params should decode");
                    let waiter = self
                        .state
                        .hook_waiter
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                        .expect("one hook waiter should be configured");
                    if let Some(started) = self
                        .state
                        .hook_started
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let _ = started.send(request_id.clone());
                    }
                    Box::pin(async move {
                        let _ = waiter.await;
                        ExtensionResponse::success(
                            request_id,
                            BeforeTurnHookResult::Continue {
                                items: params.items,
                            },
                        )
                        .map_err(|_| ExtensionTransportError::Protocol)
                    })
                }
                ExtensionMethod::HooksCancel => {
                    let params = request
                        .decode_params::<extension_protocol::HookCancelParams>()
                        .expect("cancel params should decode");
                    self.state
                        .cancel_targets
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(params.request_id);
                    let response =
                        ExtensionResponse::success(request_id, HookCancelResult { accepted: true })
                            .expect("response should encode");
                    Box::pin(async move { Ok(response) })
                }
                _ => unreachable!("blocking fixture only serves before_turn lifecycle"),
            }
        }

        fn shutdown(&self) -> Result<(), ExtensionTransportError> {
            self.state.shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct BlockingHookSource {
        state: Arc<BlockingHookState>,
    }

    impl ExtensionBundleSource for BlockingHookSource {
        fn discover(&self) -> Result<extension_runtime::ExtensionBundle, ExtensionDiscoveryError> {
            let transport = BlockingHookTransport {
                state: Arc::clone(&self.state),
            };
            let bundle = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| {
                    ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable)
                })?
                .block_on(
                    ExtensionClient::new(transport, ExtensionOptions::default()).discover(),
                )?;
            Ok(bundle.with_rediscovery_source(Arc::new(self.clone())))
        }
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
        owned_grants: Option<AgentRuntimeConstructionGrants>,
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
                owned_grants: None,
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
                owned_grants: None,
                is_shutdown: true,
                is_finalized: false,
            }
        }

        fn with_transient_shutdown_failure(lifecycle_trace: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: false,
                shutdown_failures_remaining: 1,
                shutdown_calls: None,
                drop_count: None,
                owned_grants: None,
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
                owned_grants: None,
                is_shutdown: true,
                is_finalized: false,
            }
        }

        fn with_abort_probes(
            lifecycle_trace: Arc<Mutex<Vec<&'static str>>>,
            grants: AgentRuntimeConstructionGrants,
            drop_count: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                lifecycle_trace,
                activation_failure: false,
                shutdown_failures_remaining: 0,
                shutdown_calls: None,
                drop_count: Some(drop_count),
                owned_grants: Some(grants),
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
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
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
            let _ = self.owned_grants.take();
            if let Some(drop_count) = &self.drop_count {
                drop_count.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    struct DropCounter {
        count: Arc<AtomicUsize>,
    }

    #[derive(Default)]
    struct ChildAcceptingRuntime {
        is_shutdown: bool,
        events: Vec<AgentEvent>,
        shutdown_calls: Option<Arc<AtomicUsize>>,
    }

    impl AgentRuntime for ChildAcceptingRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            if self.is_shutdown {
                return Err(AgentRuntimeError::Disposed);
            }
            match command {
                AgentCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request,
                } => {
                    let target = request.target();
                    self.events.push(AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "child complete",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    });
                    Ok(AgentCommandReceipt::TurnStarted {
                        turn_id,
                        target,
                        activity_label: request.activity_label().to_string(),
                    })
                }
                AgentCommand::Interrupt { target, .. } => {
                    Ok(AgentCommandReceipt::Interrupted { target })
                }
                AgentCommand::RespondPermission { .. } => Ok(AgentCommandReceipt::Accepted),
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.events)
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            if !self.is_shutdown
                && let Some(shutdown_calls) = &self.shutdown_calls
            {
                shutdown_calls.fetch_add(1, Ordering::SeqCst);
            }
            self.is_shutdown = true;
            Ok(())
        }
    }

    impl AgentRuntimePort for ChildAcceptingRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            self.is_shutdown = false;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
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
            !self.events.is_empty()
        }
    }

    struct SpawnParentState {
        tools: Mutex<Option<ToolExecutorRegistry>>,
        target: Mutex<Option<runtime_domain::session::RuntimeTarget>>,
    }

    struct SpawnParentRuntime {
        state: Arc<SpawnParentState>,
        session_id: session_store::SessionId,
        is_shutdown: bool,
    }

    struct FlakyReplaySessionPort {
        inner: session_store::InMemorySessionStore,
        fail_on_append_attempt: usize,
        append_attempts: AtomicUsize,
    }

    impl FlakyReplaySessionPort {
        fn new(fail_on_append_attempt: usize) -> Self {
            Self {
                inner: session_store::InMemorySessionStore::new(),
                fail_on_append_attempt,
                append_attempts: AtomicUsize::new(0),
            }
        }

        fn append_attempts(&self) -> usize {
            self.append_attempts.load(Ordering::SeqCst)
        }
    }

    impl session_store::SessionLifecycleStore for FlakyReplaySessionPort {
        fn create_session<'a>(
            &'a self,
            header: session_store::SessionHeader,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<session_store::SessionId, session_store::SessionStoreError>,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.create_session(header)
        }

        fn append<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            item: provider_protocol::ConversationItem,
        ) -> Pin<
            Box<dyn Future<Output = Result<String, session_store::SessionStoreError>> + Send + 'a>,
        > {
            self.inner.append(session_id, item)
        }

        fn append_many<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            items: Vec<provider_protocol::ConversationItem>,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<Vec<String>, session_store::SessionStoreError>>
                    + Send
                    + 'a,
            >,
        > {
            self.inner.append_many(session_id, items)
        }

        fn append_config_change<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            snapshot: session_store::ConfigSnapshot,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            self.inner.append_config_change(session_id, snapshot)
        }

        fn append_transcript_replay<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            item: runtime_domain::session::TranscriptReplayItem,
        ) -> Pin<
            Box<dyn Future<Output = Result<String, session_store::SessionStoreError>> + Send + 'a>,
        > {
            let attempt = self.append_attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == self.fail_on_append_attempt {
                return Box::pin(async {
                    Err(session_store::SessionStoreError::ConfigurationError {
                        message: "PRIVATE_REPLAY_APPEND_FAILURE".to_string(),
                    })
                });
            }
            self.inner.append_transcript_replay(session_id, item)
        }

        fn set_leaf<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            self.inner.set_leaf(session_id, leaf_id)
        }

        fn resolve<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            Vec<provider_protocol::ConversationItem>,
                            session_store::SessionStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.resolve(session_id, leaf_id)
        }

        fn load_session<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
            leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<
                            session_store::ResolvedSessionState,
                            session_store::SessionStoreError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            self.inner.load_session(session_id, leaf_id)
        }
    }

    impl session_store::SessionFlushStore for FlakyReplaySessionPort {
        fn flush<'a>(
            &'a self,
            session_id: &'a session_store::SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            self.inner.flush(session_id)
        }

        fn flush_all<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<(), session_store::SessionStoreError>> + Send + 'a>>
        {
            self.inner.flush_all()
        }
    }

    impl AgentRuntime for SpawnParentRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            if self.is_shutdown {
                return Err(AgentRuntimeError::Disposed);
            }
            match command {
                AgentCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request,
                } if agent_id == AgentId::MAIN => {
                    let target = request.target();
                    *self
                        .state
                        .target
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(target.clone());
                    Ok(AgentCommandReceipt::TurnStarted {
                        turn_id,
                        target,
                        activity_label: request.activity_label().to_string(),
                    })
                }
                _ => Err(AgentRuntimeError::UnknownAgent),
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            Vec::new()
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
            Ok(())
        }
    }

    impl AgentRuntimePort for SpawnParentRuntime {
        fn activate(&mut self, mut grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            if let Some(tools) = grants.take_tools() {
                self.bind_tools(tools)?;
            }
            self.is_shutdown = false;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
            Ok(())
        }

        fn bind_tools(
            &mut self,
            tools: crate::runtime::agent_capability_context::AgentScopedToolView,
        ) -> Result<(), String> {
            *self
                .state
                .tools
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(
                tools
                    .construction_registry()
                    .map_err(|error| error.to_string())?,
            );
            Ok(())
        }

        fn activity(&self) -> AgentRuntimeActivity {
            AgentRuntimeActivity::Idle
        }

        fn session(&self) -> Option<&dyn AgentSessionCapability> {
            Some(self)
        }

        fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability> {
            Some(self)
        }

        fn current_target(&self) -> Option<runtime_domain::session::RuntimeTarget> {
            self.state
                .target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn has_pending_work(&self) -> bool {
            false
        }
    }

    impl AgentSessionCapability for SpawnParentRuntime {
        fn snapshot(&self) -> crate::runtime::agent::AgentSessionSnapshot {
            crate::runtime::agent::AgentSessionSnapshot {
                session_id: Some(self.session_id.clone()),
                is_history_empty: false,
            }
        }

        fn truncate_after_user_turns(
            &mut self,
            _retained_user_turns: usize,
        ) -> Result<Option<(session_store::SessionId, String)>, String> {
            Ok(None)
        }

        fn context_budget_snapshot(&self) -> crate::runtime::agent::AgentContextBudgetSnapshot {
            crate::runtime::agent::AgentContextBudgetSnapshot {
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
        ) -> crate::runtime::agent::AgentEmptySessionConfigurationOutcome {
            crate::runtime::agent::AgentEmptySessionConfigurationOutcome::DeferredToNextSession
        }

        fn restore_session(&mut self, _restore: AgentSessionRestore) -> Result<(), String> {
            Ok(())
        }
    }

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn child_spawn_commits_identity_index_and_terminal_projection() {
        let factory = AgentRuntimeFactory::with_child_constructor(
            construct_native_agent_runtime,
            |_grants| Ok(Box::new(ChildAcceptingRuntime::default())),
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize with child factory");
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "qwen3", "delivery objective"),
        );
        let title = runtime_domain::agent::AgentTitle::resolve(
            &runtime_domain::agent::AgentObjective::new("delivery objective")
                .expect("objective should be valid"),
            None,
        )
        .expect("title should resolve");
        let (child_id, receipt) = components
            .spawn_child_agent(
                AgentId::MAIN,
                AgentTurnId::new(41),
                title,
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
                request,
            )
            .expect("child spawn should commit and dispatch");

        assert_eq!(child_id, AgentId::new(2));
        assert!(matches!(receipt, AgentCommandReceipt::TurnStarted { .. }));
        assert_eq!(
            components.agent_orchestrator.children_of(AgentId::MAIN),
            vec![child_id]
        );
        let events = components.drain_child_agent_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].agent_id, child_id);
        assert_eq!(
            components.agent_orchestrator.child_status(child_id),
            Some(runtime_domain::agent::AgentProjectionStatus::Completed)
        );
        assert!(matches!(
            components.dispatch_child_agent(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target: None,
            }),
            Err(AgentRuntimeError::UnknownAgent)
        ));

        components.shutdown().expect("runtime should shut down");
    }

    #[tokio::test]
    async fn scoped_spawn_agents_commits_redacted_launch_and_outcome_in_order() {
        const PRIVATE_SECOND_LINE: &str = "PRIVATE_SECOND_LINE";
        const PRIVATE_INSTRUCTIONS: &str = "PRIVATE_INSTRUCTIONS";

        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut header = session_store::SessionHeader {
            session_id: session_store::SessionId::new(),
            work_dir: std::path::PathBuf::from("/typed-spawn-session"),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        };
        let session_id = store
            .create_session(header.clone())
            .await
            .expect("spawn fixture session should be created");
        header.session_id = session_id.clone();

        let parent_state = Arc::new(SpawnParentState {
            tools: Mutex::new(None),
            target: Mutex::new(None),
        });
        let parent_session_id = session_id.clone();
        let factory = AgentRuntimeFactory::with_child_constructor(
            {
                let parent_state = Arc::clone(&parent_state);
                move |_grants| {
                    Ok(Box::new(SpawnParentRuntime {
                        state: Arc::clone(&parent_state),
                        session_id: parent_session_id.clone(),
                        is_shutdown: true,
                    }))
                }
            },
            |_grants| Ok(Box::new(ChildAcceptingRuntime::default())),
        );
        let mut options = AppRuntimeOptions {
            session_store: Some(store.clone()),
            session_header_template: Some(header),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize with typed spawn fixtures");

        let parent_turn_id = AgentTurnId::new(77);
        components
            .dispatch_main_agent(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: parent_turn_id,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "parent turn"),
                )),
            })
            .expect("parent turn should establish current spawn identity");
        let scoped_tools = parent_state
            .tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("main activation should bind scoped tools");
        let runtime_generation = components.agent_orchestrator.generation().get();

        let execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            scoped_tools
                .execute_tool_with_context(
                    ToolCall::new(
                        "spawn-call",
                        "spawn_agents",
                        serde_json::json!({
                            "agents": [{
                                "objective": format!("first delivery line\n{PRIVATE_SECOND_LINE}"),
                                "display_title": "child title",
                                "instructions": PRIVATE_INSTRUCTIONS
                            }]
                        }),
                    ),
                    ToolExecutionContext::new(&cancellation).with_invocation_identity(
                        ToolInvocationIdentity::new(
                            AgentId::MAIN.get(),
                            parent_turn_id.get(),
                            runtime_generation,
                            u64::MAX,
                        ),
                    ),
                )
                .await
        });

        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if components.child_agent_count_for_test() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(components.child_agent_count_for_test(), 1);
        let child_events = components.drain_child_agent_events();
        assert_eq!(child_events.len(), 1);
        assert!(matches!(
            child_events[0].kind,
            AgentEventKind::TurnFinished { .. }
        ));

        // document facts 与 durable replay facts 同源同序：launch 先于 outcome。
        let projection_events = components.drain_agent_projection_events();
        let projected_launch = match projection_events.first() {
            Some(AgentProjectionEvent::AgentLaunchFact { snapshot }) => snapshot.clone(),
            other => panic!("expected a launch document fact first, got {other:?}"),
        };
        let projected_outcome = match projection_events.get(1) {
            Some(AgentProjectionEvent::AgentOutcomeFact { snapshot }) => snapshot.clone(),
            other => panic!("expected an outcome document fact second, got {other:?}"),
        };
        assert_eq!(
            projection_events.len(),
            2,
            "no observation surface is registered, only document facts should project"
        );

        let tool_result = execution
            .await
            .expect("spawn tool task should finish after group completion");
        assert_eq!(tool_result.outcome(), ToolResultOutcome::Success);
        let completion_text = tool_result.text_content();
        let completion: AgentGroupCompletion = serde_json::from_str(&completion_text)
            .expect("spawn tool should return typed group completion JSON");
        assert_eq!(completion.parent_agent_id, AgentId::MAIN);
        assert_eq!(completion.children.len(), 1);
        assert_eq!(completion.children[0].outcome, AgentOutcome::Completed);
        assert!(!completion_text.contains(PRIVATE_SECOND_LINE));
        assert!(!completion_text.contains(PRIVATE_INSTRUCTIONS));

        let restored = store
            .load_session(&session_id, None)
            .await
            .expect("spawn replay facts should load from the session store");
        assert_eq!(restored.transcript.len(), 2);
        let launch = match &restored.transcript[0] {
            runtime_domain::session::TranscriptReplayItem::AgentLaunch(snapshot) => snapshot,
            other => panic!("expected launch fact first, got {other:?}"),
        };
        assert_eq!(launch.parent_agent_id, AgentId::MAIN);
        assert_eq!(launch.parent_turn_id, parent_turn_id);
        assert_eq!(launch.children.len(), 1);
        assert_eq!(launch.children[0].title.as_str(), "child title");
        assert_eq!(launch.children[0].objective.as_str(), "first delivery line");
        let outcome = match &restored.transcript[1] {
            runtime_domain::session::TranscriptReplayItem::AgentOutcome(snapshot) => snapshot,
            other => panic!("expected outcome fact second, got {other:?}"),
        };
        assert_eq!(outcome.agent_id, launch.children[0].agent_id);
        assert_eq!(outcome.group_id, Some(launch.group_id));
        assert_eq!(outcome.parent_agent_id, Some(AgentId::MAIN));
        assert_eq!(outcome.parent_turn_id, Some(parent_turn_id));
        assert_eq!(outcome.outcome, AgentOutcome::Completed);
        // document 投影与 durable replay fact 是同一份 typed snapshot。
        assert_eq!(projected_launch, *launch);
        assert_eq!(projected_outcome, *outcome);
        let replay_json = serde_json::to_string(&restored.transcript)
            .expect("replay projection should remain serializable");
        assert!(!replay_json.contains(PRIVATE_SECOND_LINE));
        assert!(!replay_json.contains(PRIVATE_INSTRUCTIONS));

        components.shutdown().expect("runtime should shut down");
    }

    /// 提交 turn 时先发出 permission request，respond 后继续 tool activity 并 terminal。
    struct PermissionChildRuntime {
        events: Vec<AgentEvent>,
        is_shutdown: bool,
    }

    impl AgentRuntime for PermissionChildRuntime {
        fn dispatch(
            &mut self,
            command: AgentCommand,
        ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
            if self.is_shutdown {
                return Err(AgentRuntimeError::Disposed);
            }
            match command {
                AgentCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request,
                } => {
                    let target = request.target();
                    self.events.push(AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::PermissionRequested {
                            request: child_permission_request("perm-1"),
                        },
                    });
                    Ok(AgentCommandReceipt::TurnStarted {
                        turn_id,
                        target,
                        activity_label: request.activity_label().to_string(),
                    })
                }
                AgentCommand::RespondPermission {
                    agent_id, target, ..
                } => {
                    let Some(target) = target else {
                        return Err(AgentRuntimeError::CommandRejected(
                            "permission target is required".to_string(),
                        ));
                    };
                    // launch convention：child turn id 由 child agent id 派生。
                    let turn_id = AgentTurnId::new(agent_id.get());
                    self.events.push(AgentEvent {
                        agent_id,
                        turn_id,
                        target: target.clone(),
                        kind: AgentEventKind::ToolActivityStarted {
                            activity: runtime_domain::session::RuntimeToolActivity {
                                activity_id: "child-tool".to_string(),
                                title: "Read file".to_string(),
                                kind: runtime_domain::session::RuntimeToolKind::Read,
                                status:
                                    runtime_domain::session::RuntimeToolActivityStatus::InProgress,
                                content: vec![
                                    runtime_domain::session::RuntimeToolActivityContent::Text(
                                        "safe child tool content".to_string(),
                                    ),
                                ],
                                locations: Vec::new(),
                                raw_input: None,
                                raw_output: None,
                            },
                        },
                    });
                    self.events.push(AgentEvent {
                        agent_id,
                        turn_id,
                        target,
                        kind: AgentEventKind::TurnFinished {
                            response: runtime_domain::session::ConversationResponse::assistant_text(
                                "child answer",
                            ),
                            metrics: None,
                            context_usage: None,
                        },
                    });
                    Ok(AgentCommandReceipt::Accepted)
                }
                AgentCommand::Interrupt { target, .. } => {
                    Ok(AgentCommandReceipt::Interrupted { target })
                }
            }
        }

        fn drain_events(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.events)
        }

        fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
            Ok(())
        }
    }

    impl AgentRuntimePort for PermissionChildRuntime {
        fn activate(&mut self, _grants: AgentRuntimeActivationGrants) -> Result<(), String> {
            self.is_shutdown = false;
            Ok(())
        }

        fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
            self.is_shutdown = true;
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
            !self.events.is_empty()
        }
    }

    fn child_permission_request(
        request_id: &str,
    ) -> runtime_domain::session::RuntimePermissionRequest {
        runtime_domain::session::RuntimePermissionRequest::new(
            request_id,
            Some("Run shell command".to_string()),
            vec![
                runtime_domain::session::RuntimePermissionOption::new(
                    "allow-1",
                    "Allow once",
                    runtime_domain::session::RuntimePermissionOptionKind::AllowOnce,
                ),
                runtime_domain::session::RuntimePermissionOption::new(
                    "reject-1",
                    "Reject once",
                    runtime_domain::session::RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        )
    }

    #[tokio::test]
    async fn observation_and_permission_ports_project_child_lifecycle_end_to_end() {
        let store = Arc::new(session_store::InMemorySessionStore::new());
        let mut header = session_store::SessionHeader {
            session_id: session_store::SessionId::new(),
            work_dir: std::path::PathBuf::from("/observation-ports-session"),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        };
        let session_id = store
            .create_session(header.clone())
            .await
            .expect("observation fixture session should be created");
        header.session_id = session_id.clone();

        let parent_state = Arc::new(SpawnParentState {
            tools: Mutex::new(None),
            target: Mutex::new(None),
        });
        let parent_session_id = session_id.clone();
        let factory = AgentRuntimeFactory::with_child_constructor(
            {
                let parent_state = Arc::clone(&parent_state);
                move |_grants| {
                    Ok(Box::new(SpawnParentRuntime {
                        state: Arc::clone(&parent_state),
                        session_id: parent_session_id.clone(),
                        is_shutdown: true,
                    }))
                }
            },
            |_grants| {
                Ok(Box::new(PermissionChildRuntime {
                    events: Vec::new(),
                    is_shutdown: true,
                }))
            },
        );
        let mut options = AppRuntimeOptions {
            session_store: Some(store.clone()),
            session_header_template: Some(header),
            ..options_with_provider()
        };
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize with observation fixtures");

        let parent_turn_id = AgentTurnId::new(88);
        components
            .dispatch_main_agent(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: parent_turn_id,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "parent turn"),
                )),
            })
            .expect("parent turn should establish spawn identity");
        let scoped_tools = parent_state
            .tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("main activation should bind scoped tools");
        let runtime_generation = components.agent_orchestrator.generation().get();

        let execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            scoped_tools
                .execute_tool_with_context(
                    ToolCall::new(
                        "spawn-call",
                        "spawn_agents",
                        serde_json::json!({
                            "agents": [{
                                "objective": "write a haiku about ports",
                                "instructions": "PRIVATE_INSTRUCTIONS"
                            }]
                        }),
                    ),
                    ToolExecutionContext::new(&cancellation).with_invocation_identity(
                        ToolInvocationIdentity::new(
                            AgentId::MAIN.get(),
                            parent_turn_id.get(),
                            runtime_generation,
                            u64::MAX,
                        ),
                    ),
                )
                .await
        });
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if components.child_agent_count_for_test() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(components.child_agent_count_for_test(), 1);
        let child_id = components
            .agent_orchestrator
            .children_of(AgentId::MAIN)
            .first()
            .copied()
            .expect("spawned child id should be indexed");

        // observe：snapshot 先行，request id 原样回显。
        components.observe_agents(runtime_domain::agent::AgentObservationRequestId::new(31));
        components.observe_agent_transcript(
            runtime_domain::agent::AgentObservationRequestId::new(32),
            child_id,
        );
        let projection = components.drain_agent_projection_events();
        let overview_snapshot = projection
            .iter()
            .find_map(|event| match event {
                runtime_domain::agent::AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
                    request_id,
                    snapshot,
                } => Some((*request_id, snapshot.clone())),
                _ => None,
            })
            .expect("overview snapshot should be delivered");
        assert_eq!(
            overview_snapshot.0,
            runtime_domain::agent::AgentObservationRequestId::new(31)
        );
        let view_snapshot = projection
            .iter()
            .find_map(|event| match event {
                runtime_domain::agent::AgentProjectionEvent::AgentViewSnapshotLoaded {
                    snapshot,
                    ..
                } => Some(snapshot.clone()),
                _ => None,
            })
            .expect("per-agent snapshot should be delivered");
        // launch 冻结 delivery-safe user objective；instructions 不进入 transcript。
        assert_eq!(
            view_snapshot.transcript.items,
            vec![runtime_domain::agent::AgentTranscriptItem::User {
                content: "write a haiku about ports".to_string()
            }]
        );

        // child fact：permission 进入 FIFO 并独立于 observation 投影 pending head。
        let child_events = components.drain_child_agent_events();
        assert_eq!(child_events.len(), 1);
        assert!(matches!(
            child_events[0].kind,
            AgentEventKind::PermissionRequested { .. }
        ));
        let projection = components.drain_agent_projection_events();
        let permission_target = projection
            .iter()
            .find_map(|event| match event {
                runtime_domain::agent::AgentProjectionEvent::AgentPermissionUpdated { update } => {
                    update
                        .request
                        .as_ref()
                        .map(|request| request.target.clone())
                }
                _ => None,
            })
            .expect("pending permission head should be projected");
        assert_eq!(permission_target.agent_id, child_id);
        assert_eq!(
            permission_target.generation,
            components.agent_orchestrator.generation()
        );
        assert_eq!(
            permission_target.runtime_target,
            runtime_domain::session::RuntimeTarget::provider("local", "qwen3")
        );

        // respond：typed response 路由到正确 child，收敛后 head 推进。
        components
            .respond_child_agent_permission(permission_target.clone(), Some("allow-1".to_string()))
            .expect("valid permission response should route to the child");
        assert!(
            components
                .respond_child_agent_permission(
                    permission_target.clone(),
                    Some("allow-1".to_string())
                )
                .is_err(),
            "duplicate submission must fail closed"
        );
        let projection = components.drain_agent_projection_events();
        assert!(projection.iter().any(|event| matches!(
            event,
            runtime_domain::agent::AgentProjectionEvent::AgentPermissionUpdated { update }
                if update.request.as_ref().is_some_and(|request| request.state
                    == runtime_domain::agent::AgentPermissionState::Submitted)
        )));

        // terminal：committed transcript/preview/overview 投影与 group completion。
        let terminal_child_events = components.drain_child_agent_events();
        assert_eq!(terminal_child_events.len(), 2);
        assert!(terminal_child_events[1].kind.is_terminal());
        let projection = components.drain_agent_projection_events();
        let final_view = projection
            .iter()
            .filter_map(|event| match event {
                runtime_domain::agent::AgentProjectionEvent::AgentViewUpdated { snapshot } => {
                    Some(snapshot.clone())
                }
                _ => None,
            })
            .next_back()
            .expect("view observation should receive updated snapshots");
        assert_eq!(
            final_view.transcript.items,
            vec![
                runtime_domain::agent::AgentTranscriptItem::User {
                    content: "write a haiku about ports".to_string()
                },
                runtime_domain::agent::AgentTranscriptItem::Tool {
                    title: "Read file".to_string(),
                    content: "safe child tool content".to_string(),
                },
                runtime_domain::agent::AgentTranscriptItem::Assistant {
                    content: "child answer".to_string()
                },
            ]
        );
        assert_eq!(
            final_view.preview.latest_committed_answer,
            Some("child answer".to_string())
        );
        assert_eq!(final_view.preview.permission, None);
        assert_eq!(
            final_view.preview.status,
            runtime_domain::agent::AgentProjectionStatus::Completed
        );
        assert!(
            projection.iter().any(|event| matches!(
                event,
                runtime_domain::agent::AgentProjectionEvent::AgentPermissionUpdated { update }
                    if update.request.is_none()
            )),
            "converged permission head should project None"
        );

        let tool_result = execution
            .await
            .expect("spawn tool task should finish after group completion");
        assert_eq!(tool_result.outcome(), ToolResultOutcome::Success);

        // typed stop 的 generation 校验 fail closed；launch-group child 的 settled row 由
        // session transition 统一回收——此时 observation 已失效，不再产生任何 fresh delta
        //（Remove 到 live observer 的路径由 orchestrator 测试覆盖）。
        assert!(
            components
                .stop_child_agent_with_generation(
                    child_id,
                    runtime_domain::agent::AgentRuntimeGeneration::new(
                        runtime_generation.saturating_add(1)
                    ),
                )
                .is_err(),
            "stale generation stop must fail closed"
        );
        components
            .dispose_child_agents_for_session_transition()
            .expect("session transition should retire the settled child projection");
        assert_eq!(components.child_agent_count_for_test(), 0);
        assert!(
            components.drain_agent_projection_events().is_empty(),
            "invalidated observations must not receive fresh deltas"
        );

        components.shutdown().expect("runtime should shut down");
    }

    #[tokio::test]
    async fn launch_fact_failure_rolls_back_staged_child_and_allows_clean_retry() {
        let replay_port = Arc::new(FlakyReplaySessionPort::new(0));
        let header = session_store::SessionHeader {
            session_id: session_store::SessionId::new(),
            work_dir: std::path::PathBuf::from("/typed-spawn-launch-retry"),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        };
        let session_id = replay_port
            .create_session(header)
            .await
            .expect("launch retry fixture session should be created");
        let parent_state = Arc::new(SpawnParentState {
            tools: Mutex::new(None),
            target: Mutex::new(None),
        });
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let factory = AgentRuntimeFactory::with_child_constructor(
            {
                let parent_state = Arc::clone(&parent_state);
                let session_id = session_id.clone();
                move |_grants| {
                    Ok(Box::new(SpawnParentRuntime {
                        state: Arc::clone(&parent_state),
                        session_id: session_id.clone(),
                        is_shutdown: true,
                    }))
                }
            },
            {
                let shutdown_calls = Arc::clone(&shutdown_calls);
                move |_grants| {
                    Ok(Box::new(ChildAcceptingRuntime {
                        shutdown_calls: Some(Arc::clone(&shutdown_calls)),
                        ..ChildAcceptingRuntime::default()
                    }))
                }
            },
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize for launch rollback");
        components
            .agent_orchestrator
            .bind_session_port(Some(replay_port.clone()));
        let parent_turn_id = AgentTurnId::new(78);
        components
            .dispatch_main_agent(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: parent_turn_id,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "parent turn"),
                )),
            })
            .expect("parent turn should start");
        let scoped_tools = parent_state
            .tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("main activation should bind scoped tools");
        let runtime_generation = components.agent_orchestrator.generation().get();

        let first_execution = {
            let scoped_tools = scoped_tools.clone();
            tokio::spawn(async move {
                let cancellation = CancellationToken::new();
                scoped_tools
                    .execute_tool_with_context(
                        ToolCall::new(
                            "failed-launch",
                            "spawn_agents",
                            serde_json::json!({
                                "agents": [{
                                    "objective": "PRIVATE_FAILED_LAUNCH_OBJECTIVE",
                                    "instructions": "PRIVATE_FAILED_LAUNCH_INSTRUCTIONS"
                                }]
                            }),
                        ),
                        ToolExecutionContext::new(&cancellation).with_invocation_identity(
                            ToolInvocationIdentity::new(
                                AgentId::MAIN.get(),
                                parent_turn_id.get(),
                                runtime_generation,
                                u64::MAX,
                            ),
                        ),
                    )
                    .await
            })
        };
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if first_execution.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let first_result = first_execution
            .await
            .expect("failed launch tool task should finish");
        assert_eq!(first_result.outcome(), ToolResultOutcome::Error);
        assert_eq!(
            first_result.text_content(),
            "spawn_agents request was rejected"
        );
        assert!(!first_result.text_content().contains("PRIVATE_"));
        assert_eq!(components.child_agent_count_for_test(), 0);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);
        assert_eq!(replay_port.append_attempts(), 1);
        assert!(
            replay_port
                .load_session(&session_id, None)
                .await
                .expect("failed launch session should remain readable")
                .transcript
                .is_empty()
        );
        assert!(
            components.drain_agent_projection_events().is_empty(),
            "failed launch append must not deliver a document fact"
        );

        let retry_execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            scoped_tools
                .execute_tool_with_context(
                    ToolCall::new(
                        "retry-launch",
                        "spawn_agents",
                        serde_json::json!({
                            "agents": [{"objective": "retry delivery"}]
                        }),
                    ),
                    ToolExecutionContext::new(&cancellation).with_invocation_identity(
                        ToolInvocationIdentity::new(
                            AgentId::MAIN.get(),
                            parent_turn_id.get(),
                            runtime_generation,
                            u64::MAX,
                        ),
                    ),
                )
                .await
        });
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if components.child_agent_count_for_test() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(components.drain_child_agent_events().len(), 1);
        // retry 成功后，launch 与 outcome 两个 document facts 按 durable 顺序交付。
        let projection_events = components.drain_agent_projection_events();
        assert!(matches!(
            projection_events.first(),
            Some(AgentProjectionEvent::AgentLaunchFact { .. })
        ));
        assert!(matches!(
            projection_events.get(1),
            Some(AgentProjectionEvent::AgentOutcomeFact { .. })
        ));
        assert_eq!(projection_events.len(), 2);
        let retry_result = retry_execution
            .await
            .expect("retry launch tool task should finish");
        assert_eq!(retry_result.outcome(), ToolResultOutcome::Success);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 2);
        assert_eq!(replay_port.append_attempts(), 3);
        assert_eq!(
            replay_port
                .load_session(&session_id, None)
                .await
                .expect("retry launch session should be readable")
                .transcript
                .len(),
            2
        );

        components.shutdown().expect("runtime should shut down");
    }

    #[tokio::test]
    async fn outcome_fact_failure_retains_terminal_delivery_until_exact_retry() {
        let replay_port = Arc::new(FlakyReplaySessionPort::new(1));
        let header = session_store::SessionHeader {
            session_id: session_store::SessionId::new(),
            work_dir: std::path::PathBuf::from("/typed-spawn-outcome-retry"),
            session_name: None,
            initial_model: "qwen3".to_string(),
            git_head: None,
            cli_version: None,
        };
        let session_id = replay_port
            .create_session(header)
            .await
            .expect("outcome retry fixture session should be created");
        let parent_state = Arc::new(SpawnParentState {
            tools: Mutex::new(None),
            target: Mutex::new(None),
        });
        let factory = AgentRuntimeFactory::with_child_constructor(
            {
                let parent_state = Arc::clone(&parent_state);
                let session_id = session_id.clone();
                move |_grants| {
                    Ok(Box::new(SpawnParentRuntime {
                        state: Arc::clone(&parent_state),
                        session_id: session_id.clone(),
                        is_shutdown: true,
                    }))
                }
            },
            |_grants| Ok(Box::new(ChildAcceptingRuntime::default())),
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize for outcome retry");
        components
            .agent_orchestrator
            .bind_session_port(Some(replay_port.clone()));
        let parent_turn_id = AgentTurnId::new(79);
        components
            .dispatch_main_agent(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: parent_turn_id,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "parent turn"),
                )),
            })
            .expect("parent turn should start");
        let scoped_tools = parent_state
            .tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("main activation should bind scoped tools");
        let runtime_generation = components.agent_orchestrator.generation().get();
        let execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            scoped_tools
                .execute_tool_with_context(
                    ToolCall::new(
                        "outcome-retry",
                        "spawn_agents",
                        serde_json::json!({
                            "agents": [{"objective": "delivery"}]
                        }),
                    ),
                    ToolExecutionContext::new(&cancellation).with_invocation_identity(
                        ToolInvocationIdentity::new(
                            AgentId::MAIN.get(),
                            parent_turn_id.get(),
                            runtime_generation,
                            u64::MAX,
                        ),
                    ),
                )
                .await
        });
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if components.child_agent_count_for_test() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }

        assert!(components.drain_child_agent_events().is_empty());
        assert_eq!(replay_port.append_attempts(), 2);
        assert!(!execution.is_finished());
        // outcome append 失败时不交付 outcome document fact；已成功的 launch fact 仍保留。
        let projection_events = components.drain_agent_projection_events();
        assert!(matches!(
            projection_events.as_slice(),
            [AgentProjectionEvent::AgentLaunchFact { .. }]
        ));
        let child_id = components
            .agent_orchestrator
            .children_of(AgentId::MAIN)
            .into_iter()
            .next()
            .expect("outcome retry child should remain projected");
        let frozen_outcome = components
            .agent_orchestrator
            .pending_outcome_for_test(child_id)
            .expect("failed outcome should retain the original fact for retry");
        let terminal_events = components.drain_child_agent_events();
        assert_eq!(terminal_events.len(), 1);
        assert_eq!(replay_port.append_attempts(), 3);
        // retry 成功后交付的是同一 frozen snapshot。
        let projection_events = components.drain_agent_projection_events();
        match projection_events.as_slice() {
            [AgentProjectionEvent::AgentOutcomeFact { snapshot }] => {
                assert_eq!(*snapshot, frozen_outcome);
            }
            other => panic!("expected the retried outcome document fact, got {other:?}"),
        }
        let result = execution
            .await
            .expect("group completion should release after outcome retry");
        assert_eq!(result.outcome(), ToolResultOutcome::Success);

        assert!(components.drain_child_agent_events().is_empty());
        assert_eq!(replay_port.append_attempts(), 3);
        let restored = replay_port
            .load_session(&session_id, None)
            .await
            .expect("outcome retry session should be readable");
        assert_eq!(restored.transcript.len(), 2);
        assert!(matches!(
            restored.transcript.as_slice(),
            [
                runtime_domain::session::TranscriptReplayItem::AgentLaunch(_),
                runtime_domain::session::TranscriptReplayItem::AgentOutcome(_)
            ]
        ));
        let persisted_outcome = match &restored.transcript[1] {
            runtime_domain::session::TranscriptReplayItem::AgentOutcome(snapshot) => snapshot,
            _ => unreachable!("outcome fact should follow launch fact"),
        };
        assert_eq!(persisted_outcome, &frozen_outcome);

        components.shutdown().expect("runtime should shut down");
    }

    #[tokio::test]
    async fn batch_construction_failure_publishes_no_partial_group_and_retries_cleanly() {
        let parent_state = Arc::new(SpawnParentState {
            tools: Mutex::new(None),
            target: Mutex::new(None),
        });
        let construction_attempts = Arc::new(AtomicUsize::new(0));
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let factory = AgentRuntimeFactory::with_child_constructor(
            {
                let parent_state = Arc::clone(&parent_state);
                move |_grants| {
                    Ok(Box::new(SpawnParentRuntime {
                        state: Arc::clone(&parent_state),
                        session_id: session_store::SessionId::new(),
                        is_shutdown: true,
                    }))
                }
            },
            {
                let construction_attempts = Arc::clone(&construction_attempts);
                let shutdown_calls = Arc::clone(&shutdown_calls);
                move |_grants| {
                    if construction_attempts.fetch_add(1, Ordering::SeqCst) == 1 {
                        return Err("PRIVATE_CHILD_CONSTRUCTION_FAILURE".to_string());
                    }
                    Ok(Box::new(ChildAcceptingRuntime {
                        shutdown_calls: Some(Arc::clone(&shutdown_calls)),
                        ..ChildAcceptingRuntime::default()
                    }))
                }
            },
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory)
                .expect("runtime components should initialize for batch rollback");
        let parent_turn_id = AgentTurnId::new(80);
        components
            .dispatch_main_agent(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: parent_turn_id,
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    ConversationTurnRequest::new_user_text("local", "qwen3", "parent turn"),
                )),
            })
            .expect("parent turn should start");
        let scoped_tools = parent_state
            .tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("main activation should bind scoped tools");
        let runtime_generation = components.agent_orchestrator.generation().get();

        let failed_execution = {
            let scoped_tools = scoped_tools.clone();
            tokio::spawn(async move {
                let cancellation = CancellationToken::new();
                scoped_tools
                    .execute_tool_with_context(
                        ToolCall::new(
                            "failed-batch",
                            "spawn_agents",
                            serde_json::json!({
                                "agents": [
                                    {"objective": "first"},
                                    {"objective": "PRIVATE_SECOND_CHILD"}
                                ]
                            }),
                        ),
                        ToolExecutionContext::new(&cancellation).with_invocation_identity(
                            ToolInvocationIdentity::new(
                                AgentId::MAIN.get(),
                                parent_turn_id.get(),
                                runtime_generation,
                                u64::MAX,
                            ),
                        ),
                    )
                    .await
            })
        };
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if failed_execution.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let failed_result = failed_execution
            .await
            .expect("failed batch tool task should finish");
        assert_eq!(failed_result.outcome(), ToolResultOutcome::Error);
        assert_eq!(
            failed_result.text_content(),
            "spawn_agents request was rejected"
        );
        assert!(!failed_result.text_content().contains("PRIVATE_"));
        assert_eq!(components.child_agent_count_for_test(), 0);
        assert_eq!(construction_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);

        let retry_execution = tokio::spawn(async move {
            let cancellation = CancellationToken::new();
            scoped_tools
                .execute_tool_with_context(
                    ToolCall::new(
                        "retry-batch",
                        "spawn_agents",
                        serde_json::json!({
                            "agents": [
                                {"objective": "first retry"},
                                {"objective": "second retry"}
                            ]
                        }),
                    ),
                    ToolExecutionContext::new(&cancellation).with_invocation_identity(
                        ToolInvocationIdentity::new(
                            AgentId::MAIN.get(),
                            parent_turn_id.get(),
                            runtime_generation,
                            u64::MAX,
                        ),
                    ),
                )
                .await
        });
        for _ in 0..16 {
            components.drain_spawn_agents_requests();
            if components.child_agent_count_for_test() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(components.child_agent_count_for_test(), 2);
        assert_eq!(components.drain_child_agent_events().len(), 2);
        // sessionless（未绑定 session port）时 replay append 是 no-op Ok，
        // document facts 仍按事实顺序交付：一个 launch fact + 每个 child 一个 outcome fact。
        let projection_events = components.drain_agent_projection_events();
        assert!(matches!(
            projection_events.first(),
            Some(AgentProjectionEvent::AgentLaunchFact { .. })
        ));
        assert_eq!(
            projection_events
                .iter()
                .filter(|event| matches!(event, AgentProjectionEvent::AgentOutcomeFact { .. }))
                .count(),
            2
        );
        assert_eq!(projection_events.len(), 3);
        let retry_result = retry_execution
            .await
            .expect("retry batch tool task should finish");
        assert_eq!(retry_result.outcome(), ToolResultOutcome::Success);
        let completion: AgentGroupCompletion = serde_json::from_str(&retry_result.text_content())
            .expect("retry batch should return typed completion");
        assert_eq!(completion.children.len(), 2);
        assert_eq!(construction_attempts.load(Ordering::SeqCst), 4);
        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 3);

        components.shutdown().expect("runtime should shut down");
    }

    #[test]
    fn runtime_reset_quiesces_child_tree_before_main_replacement() {
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let child_shutdown_calls = Arc::clone(&shutdown_calls);
        let factory = AgentRuntimeFactory::with_child_constructor(
            construct_native_agent_runtime,
            move |_grants| {
                Ok(Box::new(ChildAcceptingRuntime {
                    shutdown_calls: Some(Arc::clone(&child_shutdown_calls)),
                    ..ChildAcceptingRuntime::default()
                }))
            },
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory).unwrap();
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "qwen3", "reset child"),
        );
        let title = runtime_domain::agent::AgentTitle::resolve(
            &runtime_domain::agent::AgentObjective::new("reset child").unwrap(),
            None,
        )
        .unwrap();
        components
            .spawn_child_agent(
                AgentId::MAIN,
                AgentTurnId::new(42),
                title,
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
                request,
            )
            .unwrap();

        components
            .reset_after_clear(&options)
            .expect("reset should converge child cleanup before replacement");

        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);
        assert_eq!(components.agent_orchestrator.child_count(), 0);
        assert!(components.agent_orchestrator.has_child_factory());
    }

    #[test]
    fn required_capability_revocation_quiesces_child_tree_before_provider_removal() {
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let child_shutdown_calls = Arc::clone(&shutdown_calls);
        let factory = AgentRuntimeFactory::with_child_constructor(
            construct_native_agent_runtime,
            move |_grants| {
                Ok(Box::new(ChildAcceptingRuntime {
                    shutdown_calls: Some(Arc::clone(&child_shutdown_calls)),
                    ..ChildAcceptingRuntime::default()
                }))
            },
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory).unwrap();
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "qwen3", "revoked child"),
        );
        let title = runtime_domain::agent::AgentTitle::resolve(
            &runtime_domain::agent::AgentObjective::new("revoked child").unwrap(),
            None,
        )
        .unwrap();
        components
            .spawn_child_agent(
                AgentId::MAIN,
                AgentTurnId::new(43),
                title,
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
                request,
            )
            .unwrap();

        components
            .with_lifecycle(|lifecycle, components| {
                lifecycle.deactivate_components(
                    [RUNTIME_EVENT_STREAM.component_id],
                    components,
                    ComponentLifecycleMode::Reconfigure,
                )
            })
            .expect("provider removal should wait for child cleanup");

        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);
        assert_eq!(components.agent_orchestrator.child_count(), 0);
        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Pending)
        );
    }

    #[test]
    fn plugin_replacement_quiesces_child_tree_before_committing_fresh_authority() {
        let shutdown_calls = Arc::new(AtomicUsize::new(0));
        let child_shutdown_calls = Arc::clone(&shutdown_calls);
        let factory = AgentRuntimeFactory::with_child_constructor(
            construct_native_agent_runtime,
            move |_grants| {
                Ok(Box::new(ChildAcceptingRuntime {
                    shutdown_calls: Some(Arc::clone(&child_shutdown_calls)),
                    ..ChildAcceptingRuntime::default()
                }))
            },
        );
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new_with_agent_runtime_factory(&mut options, factory).unwrap();
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "qwen3", "replace child"),
        );
        let title = runtime_domain::agent::AgentTitle::resolve(
            &runtime_domain::agent::AgentObjective::new("replace child").unwrap(),
            None,
        )
        .unwrap();
        components
            .spawn_child_agent(
                AgentId::MAIN,
                AgentTurnId::new(44),
                title,
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
                request,
            )
            .unwrap();
        let replay = ReplayFixture::new(vec![AgentEventKind::TurnInterrupted]).unwrap();

        components
            .replace_agent_with_replay_for_test(&options, replay)
            .expect("plugin replacement should wait for the child tree");

        assert_eq!(shutdown_calls.load(Ordering::SeqCst), 1);
        assert_eq!(components.agent_orchestrator.child_count(), 0);
        assert!(!components.agent_orchestrator.has_child_factory());
    }

    #[test]
    fn runtime_components_owns_alternate_agent_through_the_lifecycle_port() {
        const RECORDING_AGENT: &str = "recording-agent-loop";
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_lifecycle_trace = Arc::clone(&lifecycle_trace);
        let grant_payload_count = Arc::new(AtomicUsize::new(0));
        let observed_grant_payload_count = Arc::clone(&grant_payload_count);
        let catalog = PluginFactoryCatalog::try_new([agent_replacement_factory(
            RECORDING_AGENT,
            AgentRuntimeFactory::new(move |grants| {
                observed_grant_payload_count.store(grants.payload_count(), Ordering::SeqCst);
                Ok(Box::new(RecordingAgentRuntime::new(Arc::clone(
                    &factory_lifecycle_trace,
                ))))
            }),
        )])
        .expect("recording Agent catalog should validate");
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(RECORDING_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("recording Agent should replace Native through plugin reconciliation");
        assert_eq!(grant_payload_count.load(Ordering::SeqCst), 7);
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
        let agent_contract_source = include_str!("agent/mod.rs");
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
            "builtin_plugin_catalog_with_agent_factory(AgentRuntimeFactory::with_child_constructor(\n",
            "        construct_native_agent_runtime,\n",
            "        construct_native_child_agent_runtime,\n",
            "    ))",
        ]
        .concat();
        let native_factory_definition = ["fn construct_native_", "agent_runtime("].concat();
        let native_construction = ["NativeAgentRuntime", "::new_for_agent("].concat();
        let erased_native_owner = ["Box::new(runtime) as Box<dyn Agent", "RuntimePort>"].concat();
        let concrete_materialization = ["Self", " {"].concat();
        let plugin_factory_dispatch = [
            ".construct_agent_",
            "runtime(instance.descriptor(), grant_source)",
        ]
        .concat();
        let broad_mount = ["AgentRuntime", "Mount"].concat();
        let ignored_broad_mount = ["|_", "mount|"].concat();
        let descriptor_independent_lookup = ["prospective_", "implementation("].concat();
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
            "NativeAgentRuntime".to_string(),
            "construct_native_agent_runtime".to_string(),
            ["components.agent_", "runtime ="].concat(),
            "downcast".to_string(),
        ] {
            assert!(
                !replay_factory_source.contains(&forbidden),
                "Replay factory must not bypass plugin ownership through {forbidden}"
            );
        }

        let direct_test_owner_assignment = ["components.agent_", "runtime ="].concat();
        assert!(
            !source.contains(&direct_test_owner_assignment),
            "test adapters must enter the Agent slot through plugin reconciliation"
        );

        assert!(production_source.contains("agent_orchestrator: AgentOrchestrator"));
        assert!(!production_source.contains(&concrete_owner));
        assert!(!production_source.contains(&concrete_constructor));
        assert!(!production_source.contains(&broad_mount));
        assert!(!agent_contract_source.contains(&broad_mount));
        assert!(!agent_contract_source.contains(&ignored_broad_mount));
        assert!(!production_source.contains(&descriptor_independent_lookup));
        assert_eq!(production_source.matches(&plugin_factory_wiring).count(), 1);
        assert_eq!(
            production_source.matches(&plugin_factory_dispatch).count(),
            1
        );
        assert!(!production_source.contains(&optional_factory));
        assert!(!production_source.contains(&old_mount_check));
        assert!(!production_source.contains(&old_fresh_helper));
        assert!(!production_source.contains(&old_replacement_guard));
        assert!(!production_source.contains("AgentRuntimeConstructionInputs"));
        assert!(!production_source.contains("agent_construction_inputs"));
        assert_eq!(native_source.matches(&native_factory_definition).count(), 1);
        assert_eq!(native_source.matches(&native_construction).count(), 2);
        assert_eq!(native_source.matches(&erased_native_owner).count(), 2);
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
        assert!(production_source.contains(".activate_main(grants, root_context, child_leases)?"));
        assert!(!production_source.contains("AgentDependencyReconstruction"));
        let hook_quiescence_source = production_source
            .split_once("fn quiesce_extension_hooks(")
            .and_then(|(_, tail)| tail.split_once("fn quiesce_external_extension("))
            .map(|(method, _)| method)
            .expect("hook provider quiescence should remain inspectable");
        assert!(!hook_quiescence_source.contains(".instance("));
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
            .split_once("fn prepare_plugin_authority(")
            .and_then(|(_, tail)| tail.split_once("fn commit_plugin_authority("))
            .map(|(method, _)| method)
            .expect("authority preparation should remain a distinct callback stage");
        assert_eq!(
            prepare_authority_source
                .matches(".prospective_instance(")
                .count(),
            1
        );
        assert_eq!(
            prepare_authority_source
                .matches("construct_agent_runtime(instance, grant_source)?")
                .count(),
            1
        );
        let finalization_position = prepare_authority_source
            .find("self.finalize_agent_runtime_replacement()?")
            .expect("authority preparation must finalize the old Agent");
        let construction_position = prepare_authority_source
            .find("construct_agent_runtime(instance, grant_source)?")
            .expect("authority preparation must construct the fresh Agent");
        let projection_source_position = prepare_authority_source
            .find("self.agent_runtime_grant_source(")
            .expect("authority preparation must defer descriptor grant projection");
        assert!(
            finalization_position < projection_source_position
                && projection_source_position < construction_position,
            "old Agent finalization must precede grant projection and candidate construction"
        );

        let commit_authority_source = production_source
            .split_once("fn commit_plugin_authority(")
            .and_then(|(_, tail)| tail.split_once("fn abort_plugin_authority("))
            .map(|(method, _)| method)
            .expect("authority commit should remain a distinct callback stage");
        assert!(!commit_authority_source.contains("construct_agent_runtime("));
        for publication in [
            "self.plugins.commit_reconciliation(prepared.reconciliation)",
            "self.plugin_loader.commit_desired(prepared.desired)",
            "self.agent_orchestrator.commit_prepared_main_replacement(",
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

    fn desired_with_runtime_event_plugin_from(
        current: &DesiredPluginComposition,
        plugin_type: &'static str,
    ) -> DesiredPluginComposition {
        DesiredPluginComposition::try_new(current.iter().map(|(component_id, current_type)| {
            if component_id.as_str() == RUNTIME_EVENT_STREAM.component_id {
                (component_id.as_str(), builtin_plugin_type(plugin_type))
            } else {
                (component_id.as_str(), current_type.clone())
            }
        }))
        .expect("runtime event replacement desired state should validate")
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
                (EXTENSION_HOOKS.component_id, EXTENSION_HOOKS_PLUGIN),
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
                    .requires(EXTENSION_HOOKS.capability)
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
                ComponentDefinition::new(EXTENSION_HOOKS.component_id)
                    .implemented_by(EXTENSION_HOOKS_PLUGIN)
                    .provides(EXTENSION_HOOKS.capability),
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
    fn extension_hook_provider_replacement_rebinds_native_to_a_fresh_generation() {
        const REPLACEMENT_HOOKS: &str = "typed-extension-hooks-v2";
        let constructions = Arc::new(AtomicUsize::new(0));
        let observed_constructions = Arc::clone(&constructions);
        let mut options = options_with_provider();
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |grants| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                construct_native_agent_runtime(grants)
            }),
        )
        .expect("runtime components should initialize");
        let old_registry = components.extension_hooks.clone();
        let mut old_registration = old_registry
            .register_before_turn(
                HookOwnerId::try_new("old-owner").expect("owner id should validate"),
                HookId::try_new("old-hook").expect("hook id should validate"),
                hook_registration_options(),
                Arc::new(|payload: BeforeTurnPayload, _cancellation| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .expect("old generation hook should register");
        let old_generation = components
            .require::<ExtensionHookRegistryCapability>()
            .expect("initial hook capability should be visible")
            .generation();
        let trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&trace));
        let materializations = Arc::new(AgentRuntimeGrantMaterializationProbe::default());
        components.agent_grant_materialization_probe = Some(Arc::clone(&materializations));
        let catalog =
            PluginFactoryCatalog::try_new([extension_hooks_replacement_factory(REPLACEMENT_HOOKS)])
                .expect("replacement hook catalog should validate");
        let desired = desired_with_extension_hooks_plugin_for_test(
            components.plugin_loader.desired(),
            REPLACEMENT_HOOKS,
        );

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("hook provider replacement should succeed");
        let fresh_registry = components.extension_hooks.clone();
        components.plugin_transaction_trace = None;

        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state(EXTENSION_HOOKS.component_id),
            Some(ComponentState::Active)
        );
        assert_eq!(old_registry.snapshot().len(), 1);
        assert!(fresh_registry.snapshot().is_empty());
        assert!(components.extension_hooks.snapshot().is_empty());
        assert!(
            components
                .require::<ExtensionHookRegistryCapability>()
                .expect("fresh hook capability should be visible")
                .generation()
                > old_generation
        );
        assert_eq!(materializations.materializations(), 0);
        assert_eq!(constructions.load(Ordering::SeqCst), 1);
        let fresh_registration = fresh_registry
            .register_before_turn(
                HookOwnerId::try_new("fresh-owner").expect("owner id should validate"),
                HookId::try_new("fresh-hook").expect("hook id should validate"),
                hook_registration_options(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .expect("fresh generation hook should register");
        assert_eq!(
            components
                .agent_orchestrator
                .main_port()
                .extension_hooks_for_test()
                .expect("Native should retain the active hook lease")
                .snapshot(),
            fresh_registry.snapshot()
        );
        let trace = trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let quiesce_agent = trace
            .iter()
            .position(|event| event == "quiesce:agent_runtime")
            .expect("Native should quiesce before hook provider replacement");
        let quiesce_provider = trace
            .iter()
            .position(|event| event == "quiesce:extension_hooks")
            .expect("hook provider should quiesce after its consumer");
        let activate_provider = trace
            .iter()
            .position(|event| event == "activate:extension_hooks")
            .expect("fresh hook provider should activate");
        let activate_agent = trace
            .iter()
            .position(|event| event == "activate:agent_runtime")
            .expect("Native should reactivate after its provider");
        assert!(quiesce_agent < quiesce_provider);
        assert!(activate_provider < activate_agent);
        drop(trace);
        assert!(old_registration.dispose());
        drop(fresh_registration);
    }

    #[test]
    fn extension_hook_provider_removal_suspends_native_until_a_fresh_generation_returns() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_registry = components.extension_hooks.clone();
        let mut old_registration = old_registry
            .register_before_turn(
                HookOwnerId::try_new("old-owner").expect("owner id should validate"),
                HookId::try_new("old-hook").expect("hook id should validate"),
                hook_registration_options(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .expect("old generation hook should register");
        let old_generation = components
            .require::<ExtensionHookRegistryCapability>()
            .expect("initial hook capability should be visible")
            .generation();
        let materializations = Arc::new(AgentRuntimeGrantMaterializationProbe::default());
        components.agent_grant_materialization_probe = Some(Arc::clone(&materializations));
        let removal_trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&removal_trace));
        let desired = desired_without_extension_hooks_for_test(components.plugin_loader.desired());

        components
            .reconcile_plugin_composition(&options, desired, ComponentLifecycleMode::Reconfigure)
            .expect("hook provider removal should leave Native pending");

        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Pending)
        );
        assert_eq!(
            components.lifecycle.state(EXTENSION_HOOKS.component_id),
            None
        );
        assert!(matches!(
            components.require::<ExtensionHookRegistryCapability>(),
            Err(RuntimeContextError::MissingCapability { .. })
        ));
        assert_eq!(old_registry.snapshot().len(), 1);
        assert!(components.extension_hooks.snapshot().is_empty());
        assert!(
            components
                .agent_orchestrator
                .main_port()
                .extension_hooks_for_test()
                .is_none()
        );
        assert_eq!(materializations.materializations(), 0);
        let removal_trace = removal_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let quiesce_agent = removal_trace
            .iter()
            .position(|event| event == "quiesce:agent_runtime")
            .expect("Native should quiesce before hook provider removal");
        let quiesce_provider = removal_trace
            .iter()
            .position(|event| event == "quiesce:extension_hooks")
            .expect("hook provider should quiesce after Native");
        assert!(quiesce_agent < quiesce_provider);
        drop(removal_trace);

        let restoration_trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&restoration_trace));
        components
            .reconcile_plugin_composition(
                &options,
                builtin_desired_composition().expect("builtin desired state should validate"),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("fresh hook provider should restore Native");
        components.plugin_transaction_trace = None;

        assert_eq!(
            components.lifecycle.state(AGENT_RUNTIME_COMPONENT),
            Some(ComponentState::Active)
        );
        assert_eq!(
            components.lifecycle.state(EXTENSION_HOOKS.component_id),
            Some(ComponentState::Active)
        );
        assert!(
            components
                .require::<ExtensionHookRegistryCapability>()
                .expect("restored hook capability should be visible")
                .generation()
                > old_generation
        );
        assert_eq!(materializations.materializations(), 0);
        assert_eq!(old_registry.snapshot().len(), 1);
        assert!(components.extension_hooks.snapshot().is_empty());
        let fresh_registry = components.extension_hooks.clone();
        let fresh_registration = fresh_registry
            .register_before_turn(
                HookOwnerId::try_new("restored-owner").expect("owner id should validate"),
                HookId::try_new("restored-hook").expect("hook id should validate"),
                hook_registration_options(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .expect("restored generation hook should register");
        assert_eq!(
            components
                .agent_orchestrator
                .main_port()
                .extension_hooks_for_test()
                .expect("Native should retain the restored hook lease")
                .snapshot(),
            fresh_registry.snapshot()
        );
        let restoration_trace = restoration_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let activate_provider = restoration_trace
            .iter()
            .position(|event| event == "activate:extension_hooks")
            .expect("fresh hook provider should activate");
        let activate_agent = restoration_trace
            .iter()
            .position(|event| event == "activate:agent_runtime")
            .expect("Native should reactivate after its dependency");
        assert!(activate_provider < activate_agent);
        drop(restoration_trace);
        assert!(old_registration.dispose());
        drop(fresh_registration);
    }

    fn hook_registration_options() -> HookRegistrationOptions {
        HookRegistrationOptions::try_new(HookPriority::default(), std::time::Duration::from_secs(1))
            .expect("hook options should validate")
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
            .mount_extension_bundle(discovered_extension_bundle(first_transport))
            .expect("extension should activate after tool catalog is available");
        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Active)
        );
        let external_definition = components
            .lifecycle
            .components()
            .into_iter()
            .find(|snapshot| snapshot.id == EXTERNAL_EXTENSION_COMPONENT)
            .expect("external component should be present");
        assert_eq!(
            external_definition.required,
            [
                EXTENSION_HOOKS.capability.to_string(),
                TOOL_CATALOG.capability.to_string(),
            ]
        );
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .any(|definition| definition.name == "extension_echo")
        );
        assert_eq!(components.extension_hooks.snapshot().len(), 1);
        assert_eq!(
            components.extension_hooks.snapshot()[0].owner.as_str(),
            EXTERNAL_EXTENSION_COMPONENT
        );

        let second_transport = StaticExtensionTransport::default();
        let second_shutdowns = Arc::clone(&second_transport.shutdowns);
        components
            .mount_extension_bundle(discovered_extension_bundle(second_transport))
            .expect("a fresh mount should replace the old generation");
        assert_eq!(first_shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(second_shutdowns.load(Ordering::SeqCst), 0);
        assert_eq!(components.extension_hooks.snapshot().len(), 1);
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
            .remove_extension_bundle()
            .expect("remove should dispose extension effects");
        assert_eq!(components.external_extension_state(), None);
        assert_eq!(second_shutdowns.load(Ordering::SeqCst), 1);
        assert!(components.extension_hooks.snapshot().is_empty());
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .all(|definition| definition.name != "extension_echo")
        );
    }

    #[test]
    fn external_extension_activation_collision_rolls_back_all_new_authority() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let existing = components
            .extension_hooks
            .register_before_turn(
                HookOwnerId::try_new(EXTERNAL_EXTENSION_COMPONENT).unwrap(),
                HookId::try_new("external-hook").unwrap(),
                hook_registration_options(),
                Arc::new(|payload: BeforeTurnPayload, _| async move {
                    Ok(BeforeTurnDecision::Continue(payload))
                }),
            )
            .expect("collision fixture should register");
        let transport = StaticExtensionTransport::default();
        let shutdowns = Arc::clone(&transport.shutdowns);

        let error = components
            .mount_extension_bundle(discovered_extension_bundle(transport))
            .expect_err("hook collision should fail activation");

        assert!(!error.is_empty());
        assert!(!error.contains("external-hook"));
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(components.extension_hooks.snapshot().len(), 1);
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .all(|definition| definition.name != "extension_echo")
        );
        assert!(components.extension_mount.is_none());
        assert!(components.extension_source.is_none());
        drop(existing);
    }

    #[test]
    fn external_extension_shutdown_reverses_hooks_tools_and_transport() {
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let transport = StaticExtensionTransport::default();
        let shutdowns = Arc::clone(&transport.shutdowns);
        components
            .mount_extension_bundle(discovered_extension_bundle(transport))
            .expect("extension should activate");
        let old_hooks = components.extension_hooks.clone();
        assert_eq!(old_hooks.snapshot().len(), 1);

        components.shutdown().expect("shutdown should succeed");

        assert!(old_hooks.snapshot().is_empty());
        assert!(components.extension_hooks.snapshot().is_empty());
        assert!(components.tool_catalog.definitions().is_empty());
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
        components
            .shutdown()
            .expect("repeated shutdown should remain idempotent");
        assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn external_extension_rediscoveries_after_tool_catalog_generation_reset() {
        let mut options = options_with_provider();
        let source = Arc::new(StaticExtensionSource::default());
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");

        components
            .mount_extension_bundle(source.discover().expect("initial discovery should succeed"))
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
        assert_eq!(components.extension_hooks.snapshot().len(), 1);
    }

    #[test]
    fn external_extension_reacts_to_hook_provider_remove_and_restore() {
        let mut options = options_with_provider();
        let source = Arc::new(StaticExtensionSource::default());
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .mount_extension_bundle(source.discover().expect("initial discovery should succeed"))
            .expect("extension should activate");
        let desired_with_extension = components.plugin_loader.desired().clone();
        let old_registry = components.extension_hooks.clone();
        assert_eq!(old_registry.snapshot().len(), 1);
        let removal_trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&removal_trace));

        let without_hooks = desired_without_extension_hooks_for_test(&desired_with_extension);
        components
            .reconcile_plugin_composition(
                &options,
                without_hooks,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("hook provider removal should leave consumers pending");

        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Pending)
        );
        assert_eq!(source.discoveries.load(Ordering::SeqCst), 1);
        assert_eq!(source.shutdowns.load(Ordering::SeqCst), 1);
        assert!(old_registry.snapshot().is_empty());
        assert!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .all(|definition| definition.name != "extension_echo")
        );
        let removal_trace = removal_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let quiesce_extension = removal_trace
            .iter()
            .position(|event| event == "quiesce:externalextension")
            .expect("external extension should quiesce");
        let quiesce_provider = removal_trace
            .iter()
            .position(|event| event == "quiesce:extension_hooks")
            .expect("hook provider should quiesce");
        assert!(quiesce_extension < quiesce_provider);
        drop(removal_trace);

        let restoration_trace = Arc::new(Mutex::new(Vec::new()));
        components.plugin_transaction_trace = Some(Arc::clone(&restoration_trace));
        components
            .reconcile_plugin_composition(
                &options,
                desired_with_extension,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("fresh hook provider should restore external extension");
        components.plugin_transaction_trace = None;

        assert_eq!(
            components.external_extension_state(),
            Some(ComponentState::Active)
        );
        assert_eq!(source.discoveries.load(Ordering::SeqCst), 2);
        assert_eq!(source.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(components.extension_hooks.snapshot().len(), 1);
        assert_eq!(
            components
                .tool_catalog
                .definitions()
                .iter()
                .filter(|definition| definition.name == "extension_echo")
                .count(),
            1
        );
        let restoration_trace = restoration_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let activate_provider = restoration_trace
            .iter()
            .position(|event| event == "activate:extension_hooks")
            .expect("hook provider should activate");
        let activate_extension = restoration_trace
            .iter()
            .position(|event| event == "activate:externalextension")
            .expect("external extension should reactivate");
        assert!(activate_provider < activate_extension);
    }

    #[test]
    fn external_extension_remove_cancels_in_flight_hook_before_transport_shutdown() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (started_tx, started_rx) = mpsc::channel();
        let state = Arc::new(BlockingHookState {
            hook_waiter: Mutex::new(Some(release_rx)),
            hook_started: Mutex::new(Some(started_tx)),
            cancel_targets: Mutex::new(Vec::new()),
            shutdowns: AtomicUsize::new(0),
        });
        let source = Arc::new(BlockingHookSource {
            state: Arc::clone(&state),
        });
        let mut options = options_with_provider();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .mount_extension_bundle(source.discover().expect("discovery should succeed"))
            .expect("extension should activate");
        let registry = components.extension_hooks.clone();
        let dispatch = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("dispatch runtime should build")
                .block_on(
                    registry.dispatch_before_turn(
                        BeforeTurnPayload::try_new(vec![
                            provider_protocol::ConversationItem::text(
                                provider_protocol::Role::User,
                                "private",
                            ),
                        ])
                        .expect("payload should validate"),
                        &tokio_util::sync::CancellationToken::new(),
                    ),
                )
        });
        let invocation_request_id = started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("remote hook should enter transport");

        components
            .remove_extension_bundle()
            .expect("remove should reverse the component");
        let error = dispatch
            .join()
            .expect("dispatch thread should join")
            .expect_err("disposed hook must not deliver a result");

        assert_eq!(
            error.kind(),
            extension_hook_runtime::HookDispatchErrorKind::RegistrationDisposed
        );
        assert!(release_tx.send(()).is_err());
        assert_eq!(
            *state
                .cancel_targets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [invocation_request_id]
        );
        assert_eq!(state.shutdowns.load(Ordering::SeqCst), 1);
        assert!(components.extension_hooks.snapshot().is_empty());
        assert_eq!(components.external_extension_state(), None);
    }

    #[test]
    fn external_extension_becomes_pending_without_tool_catalog_and_does_not_rediscover() {
        let mut options = options_with_provider();
        let source = Arc::new(StaticExtensionSource::default());
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .mount_extension_bundle(source.discover().expect("initial discovery should succeed"))
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
            AgentRuntimeFactory::new(move |_grants| {
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
            AgentRuntimeFactory::new(move |_grants| {
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
    fn native_agent_factory_constructs_sessionless_children_from_scoped_context() {
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        assert!(components.agent_orchestrator.has_child_factory());
        let root = components
            .agent_orchestrator
            .root_context()
            .expect("Native activation should install a root Agent context");

        let omitted_context = root
            .child(
                AgentContextOwner::try_new("omitted-child").expect("owner should be valid"),
                AgentChildCapabilityGrants::empty(),
            )
            .expect("omitted capabilities should still form a valid child scope");
        let construction_error = match components
            .agent_orchestrator
            .construct_child(AgentId::new(2), omitted_context.clone())
        {
            Ok(_) => panic!("Native child construction must require scoped tools and prompt"),
            Err(error) => error,
        };
        assert_eq!(
            construction_error,
            "Agent plugin failed to construct its child adapter"
        );
        assert!(omitted_context.dispose().is_success());

        let child_context = root
            .child(
                AgentContextOwner::try_new("working-child").expect("owner should be valid"),
                AgentChildCapabilityGrants::empty()
                    .inherit_tools()
                    .inherit_prompt(),
            )
            .expect("inherited capabilities should form a valid child scope");
        let child_id = AgentId::new(2);
        let mut child = components
            .agent_orchestrator
            .construct_child(child_id, child_context.clone())
            .expect("Native child should be constructed from scoped capabilities");
        assert!(child.session().is_none());
        assert!(matches!(
            child.dispatch(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target: None,
            }),
            Err(AgentRuntimeError::UnknownAgent)
        ));
        assert!(matches!(
            child.dispatch(AgentCommand::Interrupt {
                agent_id: child_id,
                target: None,
            }),
            Ok(AgentCommandReceipt::Accepted)
        ));
        child.shutdown().expect("child adapter should shut down");
        assert!(child_context.dispose().is_success());
        components.shutdown().expect("runtime should shut down");
    }

    #[test]
    fn replay_agent_uses_the_plugin_owner_and_reactive_lifecycle() {
        const REPLAY_AGENT: &str = "replay-agent-loop";
        const REPLACEMENT_HOOKS: &str = "typed-extension-hooks-for-replay";
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
        let observer = Arc::clone(&options.dynamic_environment_observer);
        let observer_owners_without_agent = Arc::strong_count(&observer);
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let grant_materialization = Arc::new(AgentRuntimeGrantMaterializationProbe::default());
        components.agent_grant_materialization_probe = Some(Arc::clone(&grant_materialization));

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
        assert!(!components.agent_orchestrator.has_child_factory());
        assert_eq!(
            Arc::strong_count(&observer),
            observer_owners_without_agent,
            "Replay must not retain Native-only construction payloads"
        );
        assert_eq!(
            grant_materialization.materializations(),
            0,
            "Replay construction must not materialize Native-only grants"
        );
        let old_hook_generation = components
            .require::<ExtensionHookRegistryCapability>()
            .expect("hook capability should be visible")
            .generation();
        let hook_catalog =
            PluginFactoryCatalog::try_new([extension_hooks_replacement_factory(REPLACEMENT_HOOKS)])
                .expect("replacement hook catalog should validate");
        let desired = desired_with_extension_hooks_plugin_for_test(
            components.plugin_loader.desired(),
            REPLACEMENT_HOOKS,
        );
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &hook_catalog,
                desired,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("unobserved hook provider should replace without rebuilding Replay");
        assert!(
            components
                .require::<ExtensionHookRegistryCapability>()
                .expect("fresh hook capability should be visible")
                .generation()
                > old_hook_generation
        );
        assert_eq!(replay_lifecycle.constructions(), 1);
        assert_eq!(replay_lifecycle.activations(), 1);
        assert_eq!(
            grant_materialization.materializations(),
            0,
            "Replay must not materialize or react to an undeclared hook capability"
        );
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
            .filter_map(crate::runtime::event_mapping::runtime_event_from_main_agent_event)
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
        assert_eq!(
            grant_materialization.materializations(),
            7,
            "Native restoration should materialize exactly its declared grant groups"
        );
        assert!(components.agent_session().is_ok());
        assert!(components.agent_orchestrator.has_child_factory());
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
    fn external_agent_replacement_reacts_to_event_stream_generations() {
        const EXTERNAL_AGENT: &str = "external-agent-kernel";
        const FRESH_EVENT_STREAM: &str = "fresh-event-stream-for-external-agent";
        let source = Arc::new(ComponentKernelSource::default());
        let kernel_source: Arc<dyn AgentKernelSource> = source.clone();
        let external_catalog = PluginFactoryCatalog::try_new([external_agent_replacement_factory(
            EXTERNAL_AGENT,
            kernel_source,
        )])
        .expect("external Agent catalog should validate");
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        assert!(components.agent_session().is_ok());

        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &external_catalog,
                desired_with_agent_plugin_for_test(Some(EXTERNAL_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("external Agent should replace Native");
        assert!(!components.agent_orchestrator.has_child_factory());
        assert_eq!(source.connect_count.load(Ordering::SeqCst), 1);
        let session_error = match components.agent_session() {
            Ok(_) => panic!("external Agent must not fabricate session capability"),
            Err(error) => error,
        };
        assert_eq!(
            session_error,
            "Agent adapter does not provide session capability"
        );
        let external_component = components
            .lifecycle
            .components()
            .into_iter()
            .find(|component| component.id == AGENT_RUNTIME_COMPONENT)
            .expect("Agent component should remain declared");
        assert_eq!(
            external_component.required,
            [RUNTIME_EVENT_STREAM.capability]
        );
        assert!(external_component.optional.is_empty());

        let wakes = Arc::new(AtomicUsize::new(0));
        let captured = Arc::clone(&wakes);
        let _binding = components.runtime_event_notifier.bind_callback(move || {
            captured.fetch_add(1, Ordering::SeqCst);
        });
        components
            .agent_port_mut()
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: AgentTurnId::new(71),
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    runtime_domain::session::ConversationTurnRequest::new_user_text(
                        "external",
                        "kernel",
                        "old generation delivery",
                    ),
                )),
            })
            .expect("external Agent turn should be admitted");
        let old_generation = source.generation(0);
        assert!(old_generation.send_delta(1, 0, "old queued fact"));
        for _ in 0..200 {
            if wakes.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        let event_catalog = PluginFactoryCatalog::try_new([runtime_event_replacement_factory(
            FRESH_EVENT_STREAM,
            RuntimeComponents::activate_runtime_event_stream,
        )])
        .expect("runtime event replacement catalog should validate");
        let desired = desired_with_runtime_event_plugin_from(
            components.plugin_loader.desired(),
            FRESH_EVENT_STREAM,
        );
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &event_catalog,
                desired,
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("event-stream replacement should reactivate external Agent");
        assert_eq!(old_generation.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(source.connect_count.load(Ordering::SeqCst), 2);
        assert!(components.agent_port_mut().drain_events().is_empty());
        assert!(!old_generation.send_delta(2, 0, "stale old fact"));

        components
            .agent_port_mut()
            .dispatch(AgentCommand::SubmitTurn {
                agent_id: AgentId::MAIN,
                turn_id: AgentTurnId::new(72),
                request: Box::new(AgentTurnRequest::from_conversation_request(
                    runtime_domain::session::ConversationTurnRequest::new_user_text(
                        "external",
                        "kernel",
                        "fresh generation delivery",
                    ),
                )),
            })
            .expect("fresh external generation should admit a turn");
        let fresh_generation = source.generation(1);
        let fresh_wake_baseline = wakes.load(Ordering::SeqCst);
        assert!(fresh_generation.send_delta(1, 0, "fresh fact"));
        for _ in 0..200 {
            if wakes.load(Ordering::SeqCst) > fresh_wake_baseline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            components.agent_port_mut().drain_events().as_slice(),
            [AgentEvent {
                kind: AgentEventKind::AssistantDelta { content },
                ..
            }] if content == "fresh fact"
        ));

        let native_catalog = builtin_plugin_catalog().expect("Native catalog should validate");
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &native_catalog,
                desired_with_agent_plugin_for_test(Some(NATIVE_AGENT_RUNTIME_PLUGIN)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("Native Agent should replace external Agent");
        assert_eq!(fresh_generation.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(source.connect_count.load(Ordering::SeqCst), 2);
        assert!(components.agent_session().is_ok());
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
            .expect("Native restoration should align graph and Context");
    }

    #[test]
    fn external_agent_shutdown_reverses_connection_once() {
        const EXTERNAL_AGENT: &str = "external-agent-kernel-shutdown";
        let source = Arc::new(ComponentKernelSource::default());
        let kernel_source: Arc<dyn AgentKernelSource> = source.clone();
        let catalog = PluginFactoryCatalog::try_new([external_agent_replacement_factory(
            EXTERNAL_AGENT,
            kernel_source,
        )])
        .expect("external Agent catalog should validate");
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(EXTERNAL_AGENT)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect("external Agent should replace Native");
        let generation = source.generation(0);

        components.shutdown().expect("component shutdown");
        components.shutdown().expect("repeated component shutdown");
        assert_eq!(generation.shutdowns.load(Ordering::SeqCst), 1);
        assert_eq!(source.connect_count.load(Ordering::SeqCst), 1);
        assert!(components.agent_port_mut().drain_events().is_empty());
    }

    #[test]
    fn agent_plugin_cleanup_failure_aborts_before_candidate_construction() {
        const ALTERNATE_AGENT: &str = "alternate-agent-loop";
        let mut options = AppRuntimeOptions::default();
        let old_lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let startup_trace = Arc::clone(&old_lifecycle_trace);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |_grants| {
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
            AgentRuntimeFactory::new(move |_grants| {
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
                    AgentRuntimeFactory::new(move |grants| {
                        let _implementation_owner = &implementation_drop;
                        Ok(Box::new(RecordingAgentRuntime::with_abort_probes(
                            Arc::new(Mutex::new(Vec::new())),
                            grants,
                            Arc::clone(&constructor_candidate_drops),
                        )))
                    }),
                ))
            },
        )])
        .expect("replacement Agent catalog should validate");
        let desired = desired_with_agent_plugin_for_test(Some(ALTERNATE_AGENT));
        let agent_action = components
            .classify_agent_plugin_reconciliation(&desired)
            .expect("Agent replacement should classify");
        let reconciliation = catalog
            .prepare_reconciliation(&desired, &components.plugins)
            .expect("fresh Agent plugin should prepare");
        let agent_runtime = RuntimeComponents::prepare_agent_runtime_commit(agent_action);
        assert_eq!(Arc::strong_count(&observer), observer_owner_count);
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
            .prepare_plugin_authority(Some(&options))
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
            AgentRuntimeFactory::new(move |_grants| {
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
    fn native_factory_rejects_missing_descriptor_grant_before_authority_commit() {
        const INCOMPLETE_NATIVE: &str = "incomplete-native-agent-loop";
        let mut options = AppRuntimeOptions::default();
        let mut components =
            RuntimeComponents::new(&mut options).expect("runtime components should initialize");
        let old_desired = components.plugin_loader.desired().clone();
        let construction_attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&construction_attempts);
        let descriptor = builtin_descriptor(INCOMPLETE_NATIVE, "Incomplete Native Agent")
            .requires(RUNTIME_EVENT_STREAM.capability)
            .requires(LLM_PORT.capability)
            .requires(MODEL_CATALOG.capability)
            .requires(PERMISSION_POLICY.capability)
            .requires(PROMPT_ASSEMBLY.capability)
            .observes(SESSION_PERSISTENCE.capability);
        let catalog = PluginFactoryCatalog::try_new([agent_runtime_plugin_factory(
            descriptor,
            AgentRuntimeFactory::new(move |grants| {
                observed_attempts.fetch_add(1, Ordering::SeqCst);
                construct_native_agent_runtime(grants)
            }),
        )])
        .expect("incomplete Native catalog should validate");

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &options,
                &catalog,
                desired_with_agent_plugin_for_test(Some(INCOMPLETE_NATIVE)),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("missing tool grant must reject Native construction");

        assert_eq!(construction_attempts.load(Ordering::SeqCst), 1);
        assert!(error.contains("authority_preparation"));
        for payload in [
            "tool_catalog",
            "Agent construction grant is unavailable",
            "Incomplete Native Agent",
        ] {
            assert!(
                !error.contains(payload),
                "closed construction diagnostics must omit {payload}"
            );
        }
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
            "failed grants must not revive the finalized old Agent"
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
            AgentRuntimeFactory::new(move |_grants| {
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
            AgentRuntimeFactory::new(move |_grants| {
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
        assert!(components.finalization == RuntimeFinalization::Finalizing);
        assert!(first.contains("LifecycleExecutionError"));
        assert!(second.contains("LifecycleExecutionError"));
        assert_eq!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["activate", "shutdown", "shutdown"]
        );
    }

    #[test]
    fn repeated_shutdown_converges_after_transient_agent_cleanup_failure() {
        let mut options = AppRuntimeOptions::default();
        let lifecycle_trace = Arc::new(Mutex::new(Vec::new()));
        let factory_trace = Arc::clone(&lifecycle_trace);
        let mut components = RuntimeComponents::new_with_agent_runtime_factory(
            &mut options,
            AgentRuntimeFactory::new(move |_grants| {
                Ok(Box::new(
                    RecordingAgentRuntime::with_transient_shutdown_failure(Arc::clone(
                        &factory_trace,
                    )),
                ))
            }),
        )
        .expect("runtime should activate through the transient shutdown fixture");

        components
            .shutdown()
            .expect_err("first Agent cleanup attempt should remain pending");
        assert!(components.finalization == RuntimeFinalization::Finalizing);
        components
            .shutdown()
            .expect("second Agent cleanup attempt should converge");
        assert!(components.finalization == RuntimeFinalization::Succeeded);
        let completed_trace = lifecycle_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(completed_trace, ["activate", "shutdown", "shutdown"]);

        components
            .shutdown()
            .expect("completed shutdown should remain idempotent");
        assert_eq!(
            *lifecycle_trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            completed_trace
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
            AgentRuntimeFactory::new(move |grants| {
                observed_constructions.fetch_add(1, Ordering::SeqCst);
                construct_native_agent_runtime(grants)
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
        let rejected_options = AppRuntimeOptions::default();
        let rejected_observer = Arc::clone(&rejected_options.dynamic_environment_observer);
        let rejected_observer_owners = Arc::strong_count(&rejected_observer);

        let error = components
            .reconcile_plugin_composition_with_catalog(
                &rejected_options,
                &catalog,
                desired_with_runtime_event_plugin(INVALID_PLUGIN),
                ComponentLifecycleMode::Reconfigure,
            )
            .expect_err("graph preflight should reject duplicate capability provider");

        assert!(error.contains("graph"));
        assert_eq!(components.plugin_descriptor_snapshots(), old_descriptors);
        assert_eq!(components.plugin_loader.desired(), &old_desired);
        assert_eq!(
            Arc::strong_count(&rejected_observer),
            rejected_observer_owners,
            "graph preflight failure must not retain unpublished Agent inputs"
        );
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
            AgentRuntimeFactory::new(move |grants| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    construct_native_agent_runtime(grants)
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
            AgentRuntimeFactory::new(move |grants| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 1 {
                    Err("injected session Agent plugin mount failure".to_string())
                } else {
                    construct_native_agent_runtime(grants)
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

        assert!(error.contains("component session_persistence quiescence failed"));
        assert!(!error.contains("injected flush failure"));
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
            AgentRuntimeFactory::new(move |grants| {
                if factory_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    construct_native_agent_runtime(grants)
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

        assert!(error.contains("component llm_port graph failed"));
        assert!(!error.contains("activation epoch is exhausted"));
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

        assert!(error.contains("component agent_runtime graph failed"));
        assert!(!error.contains("activation epoch is exhausted"));
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
