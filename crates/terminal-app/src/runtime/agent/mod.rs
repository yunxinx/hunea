//! Agent loop 的内部可替换契约。

mod external;
mod native;
#[cfg(test)]
mod replay;
mod spawn_agents;
#[cfg(test)]
mod tests;

use std::{fmt, path::PathBuf, sync::Arc};

use agent_kernel_runtime::{AgentKernelSource, ExternalAgentRuntimeOptions};

#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use extension_hook_runtime::ExtensionHookRegistry;
use session_store::{ResolvedConversationState, SessionHeader, SessionId, SessionPort};
use tool_runtime::ToolExecutorRegistry;

use runtime_domain::{
    agent::{
        AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentRuntime,
        AgentRuntimeError, AgentTurnId, AgentTurnRequest,
    },
    request_policy::RuntimeRequestPolicy,
};

#[cfg(test)]
use runtime_domain::session::{ConversationTurnRequest, TranscriptUserMessage};

use crate::{
    dynamic_environment::DynamicEnvironmentObserver,
    runtime::{
        agent_capability_context::AgentCapabilityContext,
        context::{
            CapabilityGenerationGuard, CapabilityLease, ExtensionHookRegistryCapability,
            LlmPortCapability, PermissionPolicyCapability, RuntimeEventStreamCapability,
        },
        llm_port::LlmPort,
        permission_policy::PermissionPolicy,
        prompt_assembly::PromptAssemblySessionSnapshot,
    },
};

#[cfg(test)]
pub(super) use native::NativeAgentRuntime;
pub(super) use native::construct_native_agent_runtime;
pub(super) use native::construct_native_child_agent_runtime;
#[cfg(test)]
pub(super) use replay::{ReplayAgentRuntime, ReplayFixture, ReplayLifecycleProbe};
pub(super) use spawn_agents::{SpawnAgentsFailure, SpawnAgentsRequest, SpawnAgentsTool};

struct AgentPermissionConstructionGrant {
    runtime_request_policy: RuntimeRequestPolicy,
    permission_policy: PermissionPolicy,
    permission_provider_id: String,
}

/// Agent activation 只接收 descriptor 声明且由当前 Context generation 提供的 live leases。
#[derive(Default)]
pub(super) struct AgentRuntimeActivationGrants {
    event_stream: Option<CapabilityLease<RuntimeEventStreamCapability>>,
    extension_hooks: Option<CapabilityLease<ExtensionHookRegistryCapability>>,
    tools: Option<crate::runtime::agent_capability_context::AgentScopedToolView>,
}

impl AgentRuntimeActivationGrants {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn with_event_stream(
        mut self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Self {
        self.event_stream = Some(event_stream);
        self
    }

    pub(super) fn with_extension_hooks(
        mut self,
        extension_hooks: CapabilityLease<ExtensionHookRegistryCapability>,
    ) -> Self {
        self.extension_hooks = Some(extension_hooks);
        self
    }

    pub(super) fn with_tools(
        mut self,
        tools: crate::runtime::agent_capability_context::AgentScopedToolView,
    ) -> Self {
        self.tools = Some(tools);
        self
    }

    fn take_event_stream(
        &mut self,
    ) -> Result<CapabilityLease<RuntimeEventStreamCapability>, String> {
        self.event_stream.take().ok_or_else(|| {
            "Agent activation grant is unavailable: runtime_event_stream".to_string()
        })
    }

    fn take_extension_hooks(
        &mut self,
    ) -> Result<CapabilityLease<ExtensionHookRegistryCapability>, String> {
        self.extension_hooks
            .take()
            .ok_or_else(|| "Agent activation grant is unavailable: extension_hooks".to_string())
    }

    pub(super) fn take_tools(
        &mut self,
    ) -> Option<crate::runtime::agent_capability_context::AgentScopedToolView> {
        self.tools.take()
    }
}

struct AgentPromptConstructionGrant {
    dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
    hunea_config_dir: PathBuf,
    prompt_assembly: PromptAssemblySessionSnapshot,
}

struct AgentToolConstructionGrant {
    session_workspace_tools: ToolExecutorRegistry,
    prompt_assembly_tool_definitions: Vec<tool_runtime::ToolDefinition>,
}

struct AgentSessionConstructionGrant {
    session_header_template: Option<SessionHeader>,
    session_port: Option<Arc<dyn SessionPort>>,
}

/// Agent plugin factory 只能看到 descriptor 允许 host 投影的 typed construction grants。
#[derive(Default)]
pub(super) struct AgentRuntimeConstructionGrants {
    extension_hooks: Option<ExtensionHookRegistry>,
    llm_port: Option<LlmPort>,
    loaded_models: Option<conversation_runtime::models::LoadedModelCatalog>,
    permission: Option<AgentPermissionConstructionGrant>,
    prompt: Option<AgentPromptConstructionGrant>,
    tools: Option<AgentToolConstructionGrant>,
    session: Option<AgentSessionConstructionGrant>,
}

/// Native child adapter 的 typed construction input。
///
/// `AgentCapabilityContext` 是 child 的 authority owner；其 scoped views 在 construction
/// boundary 生成工具与 prompt snapshot，不能由 factory 回查 host registry。其余字段是当前
/// committed plugin generation 的 typed provider handles，不包含 session authority。
pub(super) struct AgentChildRuntimeConstructionGrants {
    pub(super) owned_agent_id: AgentId,
    pub(super) capability_context: AgentCapabilityContext,
    pub(super) event_notifier: runtime_domain::event_notifier::RuntimeEventNotifier,
    pub(super) extension_hooks: ExtensionHookRegistry,
    pub(super) llm_port: LlmPort,
    pub(super) permission_policy: PermissionPolicy,
    pub(super) permission_provider_id: String,
    pub(super) request_policy: RuntimeRequestPolicy,
    pub(super) loaded_models: conversation_runtime::models::LoadedModelCatalog,
    pub(super) dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
    pub(super) hunea_config_dir: PathBuf,
}

/// Child adapter 与 plugin generation 同步替换的 immutable host defaults。
///
/// 它不包含 tool、prompt 或 session authority；这些只能在 launch 时从
/// `AgentCapabilityContext` 的 scoped view 投影。
#[derive(Clone)]
pub(super) struct AgentChildRuntimeStaticGrants {
    request_policy: RuntimeRequestPolicy,
    loaded_models: conversation_runtime::models::LoadedModelCatalog,
    dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
    hunea_config_dir: PathBuf,
    permission_provider_id: String,
}

impl AgentChildRuntimeStaticGrants {
    pub(super) fn new(
        request_policy: RuntimeRequestPolicy,
        loaded_models: conversation_runtime::models::LoadedModelCatalog,
        dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
        hunea_config_dir: PathBuf,
        permission_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            request_policy,
            loaded_models,
            dynamic_environment_observer,
            hunea_config_dir,
            permission_provider_id: permission_provider_id.into(),
        }
    }
}

impl fmt::Debug for AgentChildRuntimeStaticGrants {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChildRuntimeStaticGrants")
            .field("has_request_policy", &true)
            .field("has_loaded_models", &true)
            .field("has_dynamic_environment_observer", &true)
            .field("has_permission_provider", &true)
            .finish_non_exhaustive()
    }
}

/// Child adapter 只保留 active component graph 当前 generation 的 typed leases。
#[derive(Clone)]
pub(super) struct AgentChildRuntimeLeases {
    event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    extension_hooks: CapabilityLease<ExtensionHookRegistryCapability>,
    llm_port: CapabilityLease<LlmPortCapability>,
    permission_policy: CapabilityLease<PermissionPolicyCapability>,
}

impl AgentChildRuntimeLeases {
    pub(super) fn new(
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
        extension_hooks: CapabilityLease<ExtensionHookRegistryCapability>,
        llm_port: CapabilityLease<LlmPortCapability>,
        permission_policy: CapabilityLease<PermissionPolicyCapability>,
    ) -> Self {
        Self {
            event_stream,
            extension_hooks,
            llm_port,
            permission_policy,
        }
    }

    pub(super) fn construction_grants(
        &self,
        owned_agent_id: AgentId,
        capability_context: AgentCapabilityContext,
        static_grants: &AgentChildRuntimeStaticGrants,
    ) -> AgentChildRuntimeConstructionGrants {
        AgentChildRuntimeConstructionGrants {
            owned_agent_id,
            capability_context,
            event_notifier: (*self.event_stream).clone(),
            extension_hooks: (*self.extension_hooks).clone(),
            llm_port: (*self.llm_port).clone(),
            permission_policy: (*self.permission_policy).clone(),
            permission_provider_id: static_grants.permission_provider_id.clone(),
            request_policy: static_grants.request_policy.clone(),
            loaded_models: static_grants.loaded_models.clone(),
            dynamic_environment_observer: Arc::clone(&static_grants.dynamic_environment_observer),
            hunea_config_dir: static_grants.hunea_config_dir.clone(),
        }
    }

    /// 为每个 child adapter 生成独立的 activation lease。
    ///
    /// lease 本身是 generation-bound 的 clone；child 不会获得 host registry 或第二份
    /// capability authority。orchestrator 在 child record 提交前调用该方法，确保 activation
    /// 失败时仍可回收已构造的 context。
    #[allow(dead_code)]
    pub(super) fn activation_grants(&self) -> AgentRuntimeActivationGrants {
        AgentRuntimeActivationGrants::empty()
            .with_event_stream(self.event_stream.clone())
            .with_extension_hooks(self.extension_hooks.clone())
    }

    /// Child tree 必须随任一 construction/activation provider generation 一同失效。
    pub(super) fn generation_guards(&self) -> [CapabilityGenerationGuard; 4] {
        [
            self.event_stream.generation_guard(),
            self.extension_hooks.generation_guard(),
            self.llm_port.generation_guard(),
            self.permission_policy.generation_guard(),
        ]
    }
}

impl fmt::Debug for AgentChildRuntimeConstructionGrants {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentChildRuntimeConstructionGrants")
            .field("has_owned_agent_id", &true)
            .field("has_capability_context", &true)
            .field("has_event_notifier", &true)
            .field("has_extension_hooks", &true)
            .field("has_llm_port", &true)
            .field("has_permission_policy", &true)
            .field("has_loaded_models", &true)
            .finish_non_exhaustive()
    }
}

impl AgentRuntimeConstructionGrants {
    pub(super) fn empty() -> Self {
        Self::default()
    }

    pub(super) fn with_extension_hooks(mut self, extension_hooks: ExtensionHookRegistry) -> Self {
        self.extension_hooks = Some(extension_hooks);
        self
    }

    pub(super) fn with_llm_port(mut self, llm_port: LlmPort) -> Self {
        self.llm_port = Some(llm_port);
        self
    }

    pub(super) fn with_loaded_models(
        mut self,
        loaded_models: conversation_runtime::models::LoadedModelCatalog,
    ) -> Self {
        self.loaded_models = Some(loaded_models);
        self
    }

    pub(super) fn with_permission(
        mut self,
        runtime_request_policy: RuntimeRequestPolicy,
        permission_policy: PermissionPolicy,
        permission_provider_id: String,
    ) -> Self {
        self.permission = Some(AgentPermissionConstructionGrant {
            runtime_request_policy,
            permission_policy,
            permission_provider_id,
        });
        self
    }

    pub(super) fn with_prompt(
        mut self,
        dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
        hunea_config_dir: PathBuf,
        prompt_assembly: PromptAssemblySessionSnapshot,
    ) -> Self {
        self.prompt = Some(AgentPromptConstructionGrant {
            dynamic_environment_observer,
            hunea_config_dir,
            prompt_assembly,
        });
        self
    }

    pub(super) fn with_tools(
        mut self,
        session_workspace_tools: ToolExecutorRegistry,
        prompt_assembly_tool_definitions: Vec<tool_runtime::ToolDefinition>,
    ) -> Self {
        self.tools = Some(AgentToolConstructionGrant {
            session_workspace_tools,
            prompt_assembly_tool_definitions,
        });
        self
    }

    pub(super) fn with_session(
        mut self,
        session_header_template: Option<SessionHeader>,
        session_port: Option<Arc<dyn SessionPort>>,
    ) -> Self {
        self.session = Some(AgentSessionConstructionGrant {
            session_header_template,
            session_port,
        });
        self
    }

    pub(super) fn payload_count(&self) -> usize {
        [
            self.extension_hooks.is_some(),
            self.llm_port.is_some(),
            self.loaded_models.is_some(),
            self.permission.is_some(),
            self.prompt.is_some(),
            self.tools.is_some(),
            self.session.is_some(),
        ]
        .into_iter()
        .filter(|is_present| *is_present)
        .count()
    }
}

impl fmt::Debug for AgentRuntimeConstructionGrants {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentRuntimeConstructionGrants")
            .field("payload_count", &self.payload_count())
            .finish_non_exhaustive()
    }
}

type AgentRuntimeConstructor = dyn Fn(AgentRuntimeConstructionGrants) -> Result<Box<dyn AgentRuntimePort>, String>
    + Send
    + Sync;

type AgentChildRuntimeConstructor = dyn Fn(AgentChildRuntimeConstructionGrants) -> Result<Box<dyn AgentRuntimePort>, String>
    + Send
    + Sync;

const AGENT_RUNTIME_CONSTRUCTION_FAILED: &str = "Agent plugin failed to construct its adapter";

/// `AgentRuntimeFactory` 是 Agent plugin implementation 独占的 adapter construction authority。
#[derive(Clone)]
pub(super) struct AgentRuntimeFactory {
    construct: Arc<AgentRuntimeConstructor>,
    child_construct: Option<Arc<AgentChildRuntimeConstructor>>,
}

impl AgentRuntimeFactory {
    pub(super) fn new(
        construct: impl Fn(AgentRuntimeConstructionGrants) -> Result<Box<dyn AgentRuntimePort>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            construct: Arc::new(construct),
            child_construct: None,
        }
    }

    pub(super) fn with_child_constructor(
        construct: impl Fn(AgentRuntimeConstructionGrants) -> Result<Box<dyn AgentRuntimePort>, String>
        + Send
        + Sync
        + 'static,
        child_construct: impl Fn(
            AgentChildRuntimeConstructionGrants,
        ) -> Result<Box<dyn AgentRuntimePort>, String>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            construct: Arc::new(construct),
            child_construct: Some(Arc::new(child_construct)),
        }
    }

    pub(super) fn construct(
        &self,
        grants: AgentRuntimeConstructionGrants,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        (self.construct)(grants).map_err(|_| AGENT_RUNTIME_CONSTRUCTION_FAILED.to_string())
    }

    pub(super) fn child_factory(&self) -> Option<ChildAgentFactory> {
        self.child_construct
            .as_ref()
            .map(|construct| ChildAgentFactory {
                construct: Arc::clone(construct),
            })
    }

    /// 为 host 显式提供的 kernel source 创建 construction authority。
    ///
    /// 默认 composition 不调用该入口；source discovery 与 trust policy 不属于本层。
    #[allow(dead_code)]
    pub(super) fn external(
        source: Arc<dyn AgentKernelSource>,
        options: ExternalAgentRuntimeOptions,
    ) -> Self {
        Self::new(move |grants| {
            if grants.payload_count() != 0 {
                return Err("External Agent received undeclared construction grants".to_string());
            }
            Ok(Box::new(external::ExternalAgentRuntimeAdapter::new(
                Arc::clone(&source),
                options,
            )))
        })
    }

    #[cfg(test)]
    pub(super) fn replay(
        fixture: ReplayFixture,
        lifecycle_probe: Option<Arc<ReplayLifecycleProbe>>,
    ) -> Self {
        Self::new(move |grants| {
            if grants.payload_count() != 0 {
                return Err("Replay Agent received undeclared construction grants".to_string());
            }
            if let Some(probe) = &lifecycle_probe {
                probe.record_construction();
            }
            let runtime = match &lifecycle_probe {
                Some(probe) => ReplayAgentRuntime::new_with_lifecycle_probe(
                    fixture.clone(),
                    runtime_domain::event_notifier::RuntimeEventNotifier::default(),
                    Arc::clone(probe),
                ),
                None => ReplayAgentRuntime::new(
                    fixture.clone(),
                    runtime_domain::event_notifier::RuntimeEventNotifier::default(),
                ),
            };
            Ok(Box::new(runtime))
        })
    }
}

/// 一个 active Agent plugin generation 提供的 typed child construction capability。
#[derive(Clone)]
pub(super) struct ChildAgentFactory {
    construct: Arc<AgentChildRuntimeConstructor>,
}

impl ChildAgentFactory {
    pub(super) fn construct(
        &self,
        grants: AgentChildRuntimeConstructionGrants,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        (self.construct)(grants)
            .map_err(|_| "Agent plugin failed to construct its child adapter".to_string())
    }
}

#[cfg(test)]
pub(super) trait AgentRuntimeTestHarness {
    fn append_conversation_items(
        &mut self,
        items: Vec<conversation_runtime::ConversationItem>,
    ) -> Result<(), String>;

    fn set_upstream_context_tokens(&mut self, upstream_context_tokens: Option<usize>);

    fn stage_pending_turn(&mut self, request: ConversationTurnRequest);

    fn set_worker_cancellation(&mut self, cancellation: tokio_util::sync::CancellationToken);

    fn prepare_turn(
        &mut self,
        request: &ConversationTurnRequest,
    ) -> Result<conversation_runtime::PreparedConversationRequest, String>;

    fn dynamic_environment_injection(
        &mut self,
        observer: Arc<dyn crate::dynamic_environment::DynamicEnvironmentObserver>,
    ) -> Result<crate::runtime::dynamic_environment_worker::DynamicEnvironmentInjection, String>;

    fn attached_prompt_message_assembly(
        &self,
        user_message: &TranscriptUserMessage,
    ) -> Result<crate::prompt_assembly::AttachedPromptMessageAssembly, String>;
}

/// Agent host 所消费的 capability port。
///
/// `RuntimeComponents` 通过该 port 唯一拥有 active adapter；`NativeAgentRuntime` 是当前唯一
/// production implementation。worker、receiver 与 event-stream lease 全部封装在 implementation
/// 内，host 只消费 lifecycle、Agent facts 与 session/configuration view。
pub(super) trait AgentRuntimePort: AgentRuntime + Send {
    /// 使用 descriptor-filtered current Context generations 激活 adapter。
    fn activate(&mut self, grants: AgentRuntimeActivationGrants) -> Result<(), String>;

    /// 撤销当前 activation generation 的副作用，同时保留可供重新激活的持久状态。
    fn suspend(&mut self) -> Result<(), AgentRuntimeError>;

    /// 绑定 host 当前 committed generation，供 scoped tools 进行 identity 校验。
    fn bind_runtime_generation(&mut self, _generation: u64) {}

    /// 绑定当前 Agent context 的 reactive tool snapshot。
    fn bind_tools(
        &mut self,
        _tools: crate::runtime::agent_capability_context::AgentScopedToolView,
    ) -> Result<(), String> {
        Ok(())
    }

    /// 返回 framework-neutral 的 Agent activity；不能由 session capability 缺省值推导。
    fn activity(&self) -> AgentRuntimeActivity;

    /// 返回当前 adapter 实际提供的 session capability。
    fn session(&self) -> Option<&dyn AgentSessionCapability>;

    /// 返回当前 adapter 实际提供的 mutable session capability。
    fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability>;

    /// 返回当前 active/pending turn 的 provider target。
    fn current_target(&self) -> Option<runtime_domain::session::RuntimeTarget> {
        None
    }

    #[cfg(test)]
    fn has_pending_work(&self) -> bool;

    #[cfg(test)]
    fn extension_hooks_for_test(&self) -> Option<ExtensionHookRegistry> {
        None
    }
}

/// Agent loop 是否仍占有一个未结束或未完成交付的 turn。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentRuntimeActivity {
    Idle,
    Busy,
}

impl AgentRuntimeActivity {
    pub(super) const fn is_busy(self) -> bool {
        matches!(self, Self::Busy)
    }
}

/// Agent-owned session state 的 immutable host projection。
pub(super) struct AgentSessionSnapshot {
    pub(super) session_id: Option<SessionId>,
    pub(super) is_history_empty: bool,
}

/// 当前空 session 是否实际接收了最新 prompt/tool configuration。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentEmptySessionConfigurationOutcome {
    Applied,
    DeferredToNextSession,
}

/// 只有实际拥有 conversation/session state 的 Agent adapter 才提供该 capability。
pub(super) trait AgentSessionCapability {
    fn snapshot(&self) -> AgentSessionSnapshot;

    fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, String>;

    fn context_budget_snapshot(&self) -> AgentContextBudgetSnapshot;

    fn update_empty_session_configuration(
        &mut self,
        prompt_assembly: crate::runtime::prompt_assembly::PromptAssemblySessionSnapshot,
        session_workspace_tools: ToolExecutorRegistry,
    ) -> AgentEmptySessionConfigurationOutcome;

    fn restore_session(&mut self, restore: AgentSessionRestore) -> Result<(), String>;

    #[cfg(test)]
    fn test_harness(&mut self) -> Option<&mut dyn AgentRuntimeTestHarness> {
        None
    }

    #[cfg(test)]
    fn test_harness_ref(&self) -> Option<&dyn AgentRuntimeTestHarness> {
        None
    }
}

/// Host 交给当前 Agent adapter 的 framework-neutral session restore value。
pub(super) struct AgentSessionRestore {
    session_port: Arc<dyn SessionPort>,
    header: SessionHeader,
    session_id: SessionId,
    conversation: ResolvedConversationState,
    #[cfg(test)]
    materialization_probe: Option<Arc<AtomicBool>>,
}

impl AgentSessionRestore {
    pub(super) fn new(
        session_port: Arc<dyn SessionPort>,
        header: SessionHeader,
        session_id: SessionId,
        conversation: ResolvedConversationState,
    ) -> Self {
        Self {
            session_port,
            header,
            session_id,
            conversation,
            #[cfg(test)]
            materialization_probe: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_materialization_probe(mut self, probe: Arc<AtomicBool>) -> Self {
        self.materialization_probe = Some(probe);
        self
    }

    fn into_parts(
        self,
    ) -> (
        Arc<dyn SessionPort>,
        SessionHeader,
        SessionId,
        ResolvedConversationState,
    ) {
        #[cfg(test)]
        if let Some(probe) = &self.materialization_probe {
            probe.store(true, Ordering::SeqCst);
        }
        (
            self.session_port,
            self.header,
            self.session_id,
            self.conversation,
        )
    }
}

impl fmt::Debug for AgentSessionRestore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentSessionRestore")
            .field("session_id", &self.session_id)
            .field("item_count", &self.conversation.items.len())
            .field(
                "has_latest_config",
                &self.conversation.latest_config.is_some(),
            )
            .finish()
    }
}

/// `/context` 与 host worker 之间传递的 Agent-owned immutable snapshot。
pub(super) struct AgentContextBudgetSnapshot {
    pub(super) items: Arc<[conversation_runtime::ConversationItem]>,
    pub(super) prompt_prelude: Option<runtime_domain::prompt_assembly::PromptPreludeSnapshot>,
    pub(super) upstream_context_tokens: Option<usize>,
    pub(super) tool_definitions: Vec<conversation_runtime::ToolDefinition>,
}
