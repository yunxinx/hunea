//! Agent loop 的内部可替换契约。

mod external;
mod native;
#[cfg(test)]
mod replay;
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
        context::{CapabilityLease, ExtensionHookRegistryCapability, RuntimeEventStreamCapability},
        llm_port::LlmPort,
        permission_policy::PermissionPolicy,
        prompt_assembly::PromptAssemblySessionSnapshot,
    },
};

#[cfg(test)]
pub(super) use native::NativeAgentRuntime;
pub(super) use native::construct_native_agent_runtime;
#[cfg(test)]
pub(super) use replay::{ReplayAgentRuntime, ReplayFixture, ReplayLifecycleProbe};

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

const AGENT_RUNTIME_CONSTRUCTION_FAILED: &str = "Agent plugin failed to construct its adapter";

/// `AgentRuntimeFactory` 是 Agent plugin implementation 独占的 adapter construction authority。
#[derive(Clone)]
pub(super) struct AgentRuntimeFactory {
    construct: Arc<AgentRuntimeConstructor>,
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
        }
    }

    pub(super) fn construct(
        &self,
        grants: AgentRuntimeConstructionGrants,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        (self.construct)(grants).map_err(|_| AGENT_RUNTIME_CONSTRUCTION_FAILED.to_string())
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

    /// 返回 framework-neutral 的 Agent activity；不能由 session capability 缺省值推导。
    fn activity(&self) -> AgentRuntimeActivity;

    /// 返回当前 adapter 实际提供的 session capability。
    fn session(&self) -> Option<&dyn AgentSessionCapability>;

    /// 返回当前 adapter 实际提供的 mutable session capability。
    fn session_mut(&mut self) -> Option<&mut dyn AgentSessionCapability>;

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
