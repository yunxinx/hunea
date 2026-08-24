//! Agent loop 的内部可替换契约。

mod native;
#[cfg(test)]
mod replay;
#[cfg(test)]
mod tests;

use std::{fmt, path::PathBuf, sync::Arc};

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
        AppRuntimeOptions,
        context::{CapabilityLease, RuntimeEventStreamCapability},
        llm_port::LlmPort,
        permission_policy::PermissionPolicy,
        prompt_assembly::PromptAssemblySessionSnapshot,
    },
};

#[cfg(test)]
pub(super) use native::NativeAgentRuntime;
pub(super) use native::construct_native_agent_runtime;

/// `AgentRuntimeMount` 是构造一个 Agent adapter generation 所需的 immutable host snapshot。
///
/// 字段只对 `runtime::agent` implementation 可见；host 只能一次性构造并交给 plugin factory，
/// 不能把它当作绕过 Context lifecycle 的 live registry。
pub(super) struct AgentRuntimeMount {
    loaded_models: conversation_runtime::models::LoadedModelCatalog,
    runtime_request_policy: RuntimeRequestPolicy,
    dynamic_environment_observer: Arc<dyn DynamicEnvironmentObserver>,
    hunea_config_dir: PathBuf,
    session_header_template: Option<SessionHeader>,
    session_workspace_tools: ToolExecutorRegistry,
    prompt_assembly_tool_definitions: Vec<tool_runtime::ToolDefinition>,
    prompt_assembly: PromptAssemblySessionSnapshot,
    session_port: Option<Arc<dyn SessionPort>>,
    llm_port: LlmPort,
    permission_policy: PermissionPolicy,
    permission_provider_id: String,
}

impl AgentRuntimeMount {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        options: &AppRuntimeOptions,
        session_workspace_tools: ToolExecutorRegistry,
        prompt_assembly_tool_definitions: Vec<tool_runtime::ToolDefinition>,
        prompt_assembly: PromptAssemblySessionSnapshot,
        session_port: Option<Arc<dyn SessionPort>>,
        llm_port: LlmPort,
        permission_policy: PermissionPolicy,
        permission_provider_id: String,
    ) -> Self {
        Self {
            loaded_models: options.loaded_models.clone(),
            runtime_request_policy: options.runtime_request_policy.clone(),
            dynamic_environment_observer: Arc::clone(&options.dynamic_environment_observer),
            hunea_config_dir: options.hunea_config_dir.clone(),
            session_header_template: options.session_header_template.clone(),
            session_workspace_tools,
            prompt_assembly_tool_definitions,
            prompt_assembly,
            session_port,
            llm_port,
            permission_policy,
            permission_provider_id,
        }
    }
}

type AgentRuntimeConstructor =
    dyn Fn(AgentRuntimeMount) -> Result<Box<dyn AgentRuntimePort>, String> + Send + Sync;

const AGENT_RUNTIME_CONSTRUCTION_FAILED: &str = "Agent plugin failed to construct its adapter";

/// `AgentRuntimeFactory` 是 Agent plugin implementation 独占的 adapter construction authority。
#[derive(Clone)]
pub(super) struct AgentRuntimeFactory {
    construct: Arc<AgentRuntimeConstructor>,
}

impl AgentRuntimeFactory {
    pub(super) fn new(
        construct: impl Fn(AgentRuntimeMount) -> Result<Box<dyn AgentRuntimePort>, String>
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
        mount: AgentRuntimeMount,
    ) -> Result<Box<dyn AgentRuntimePort>, String> {
        (self.construct)(mount).map_err(|_| AGENT_RUNTIME_CONSTRUCTION_FAILED.to_string())
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
    /// 使用当前 host event-stream generation 激活 adapter。
    fn activate(
        &mut self,
        event_stream: CapabilityLease<RuntimeEventStreamCapability>,
    ) -> Result<(), String>;

    /// 撤销当前 activation generation 的副作用，同时保留可供重新激活的持久状态。
    fn suspend(&mut self) -> Result<(), AgentRuntimeError>;

    fn is_busy(&self) -> bool;

    fn session_id(&self) -> Option<SessionId>;

    fn is_history_empty(&self) -> bool;

    fn is_idle_empty_session(&self) -> bool;

    fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, String>;

    fn context_budget_snapshot(&self) -> AgentContextBudgetSnapshot;

    fn update_empty_session_configuration(
        &mut self,
        prompt_assembly: crate::runtime::prompt_assembly::PromptAssemblySessionSnapshot,
        session_workspace_tools: ToolExecutorRegistry,
    );

    fn restore_session(&mut self, restore: AgentSessionRestore) -> Result<(), String>;

    #[cfg(test)]
    fn test_harness(&mut self) -> Option<&mut dyn AgentRuntimeTestHarness> {
        None
    }

    #[cfg(test)]
    fn test_harness_ref(&self) -> Option<&dyn AgentRuntimeTestHarness> {
        None
    }

    #[cfg(test)]
    fn has_pending_work(&self) -> bool;
}

/// Host 交给当前 Agent adapter 的 framework-neutral session restore value。
pub(super) struct AgentSessionRestore {
    session_port: Arc<dyn SessionPort>,
    header: SessionHeader,
    session_id: SessionId,
    conversation: ResolvedConversationState,
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
        }
    }

    fn into_parts(
        self,
    ) -> (
        Arc<dyn SessionPort>,
        SessionHeader,
        SessionId,
        ResolvedConversationState,
    ) {
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
