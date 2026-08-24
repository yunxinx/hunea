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
    context_budget::ContextWindowUsage,
    request_policy::RuntimeRequestPolicy,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimePermissionRequest,
        RuntimeRequestMetrics, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
        RuntimeToolActivityUpdate, TranscriptCustomPromptBinding, TranscriptSkillBinding,
        TranscriptUserAttachment, TranscriptUserMessage,
    },
};

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

/// `AgentId` 标识一个由 runtime host 管理的 Agent handle。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AgentId(u64);

impl AgentId {
    pub(super) const MAIN: Self = Self(1);

    #[cfg(test)]
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// `AgentTurnId` 标识一次 Agent turn，避免事件依赖“唯一活跃 worker”的隐式假设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AgentTurnId(u64);

impl AgentTurnId {
    pub(super) const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// `AgentUserDelivery` 只保存用户实际提交并应在 transcript 中显示的内容。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentUserDelivery {
    content: String,
    attachments: Vec<TranscriptUserAttachment>,
}

impl fmt::Debug for AgentUserDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentUserDelivery")
            .field("content_chars", &self.content.chars().count())
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// `AgentTurnControls` 保存结构化 skill/prompt 绑定，不把解析出的 instruction body
/// 混入 delivery DTO。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentTurnControls {
    skill_bindings: Vec<TranscriptSkillBinding>,
    custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
}

impl fmt::Debug for AgentTurnControls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentTurnControls")
            .field("skill_binding_count", &self.skill_bindings.len())
            .field(
                "custom_prompt_binding_count",
                &self.custom_prompt_bindings.len(),
            )
            .finish()
    }
}

/// `AgentTurnRequest` 把用户 delivery/control 与 provider target envelope 分开。
///
/// provider target envelope 只保存 provider/model identity 与 provider-visible message，
/// 不携带 credential、endpoint 或 instruction body。
#[derive(Clone, PartialEq, Eq)]
pub(super) struct AgentTurnRequest {
    delivery: AgentUserDelivery,
    controls: AgentTurnControls,
    native_request: ConversationTurnRequest,
}

impl AgentTurnRequest {
    pub(super) fn from_conversation_request(request: ConversationTurnRequest) -> Self {
        let source_message = request
            .transcript_user_message()
            .cloned()
            .unwrap_or_else(|| TranscriptUserMessage {
                content: request.message_text(),
                attachments: Vec::new(),
                skill_bindings: Vec::new(),
                custom_prompt_bindings: Vec::new(),
            });
        Self {
            delivery: AgentUserDelivery {
                content: source_message.content,
                attachments: source_message.attachments,
            },
            controls: AgentTurnControls {
                skill_bindings: source_message.skill_bindings,
                custom_prompt_bindings: source_message.custom_prompt_bindings,
            },
            native_request: request,
        }
    }

    pub(super) fn target(&self) -> RuntimeTarget {
        self.native_request.target()
    }

    pub(super) fn activity_label(&self) -> &str {
        self.native_request.model_id()
    }

    fn into_parts(self) -> (ConversationTurnRequest, TranscriptUserMessage) {
        let source_message = TranscriptUserMessage {
            content: self.delivery.content,
            attachments: self.delivery.attachments,
            skill_bindings: self.controls.skill_bindings,
            custom_prompt_bindings: self.controls.custom_prompt_bindings,
        };
        (self.native_request, source_message)
    }
}

impl fmt::Debug for AgentTurnRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentTurnRequest")
            .field("target", &self.target())
            .field("delivery", &self.delivery)
            .field("controls", &self.controls)
            .finish()
    }
}

/// `AgentCommand` 只描述 Agent loop 行为；session tree、prompt editor、model refresh
/// 与 context budget 继续由 host capability 处理。
pub(super) enum AgentCommand {
    SubmitTurn {
        agent_id: AgentId,
        turn_id: AgentTurnId,
        request: Box<AgentTurnRequest>,
    },
    Interrupt {
        agent_id: AgentId,
        target: Option<RuntimeTarget>,
    },
    RespondPermission {
        agent_id: AgentId,
        target: Option<RuntimeTarget>,
        request_id: String,
        option_id: Option<String>,
    },
}

impl fmt::Debug for AgentCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SubmitTurn {
                agent_id,
                turn_id,
                request,
            } => f
                .debug_struct("SubmitTurn")
                .field("agent_id", agent_id)
                .field("turn_id", turn_id)
                .field("request", request)
                .finish(),
            Self::Interrupt { agent_id, target } => f
                .debug_struct("Interrupt")
                .field("agent_id", agent_id)
                .field("target", target)
                .finish(),
            Self::RespondPermission {
                agent_id,
                target,
                request_id,
                option_id,
            } => f
                .debug_struct("RespondPermission")
                .field("agent_id", agent_id)
                .field("target", target)
                .field("request_id", request_id)
                .field("has_option", &option_id.is_some())
                .finish(),
        }
    }
}

/// `AgentCommandReceipt` 只确认命令已原子准入；异步执行结果通过 `AgentEvent` 交付。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentCommandReceipt {
    Accepted,
    TurnStarted {
        turn_id: AgentTurnId,
        target: RuntimeTarget,
        activity_label: String,
    },
    Interrupted {
        target: Option<RuntimeTarget>,
    },
}

/// `AgentEvent` 是带显式 Agent/turn identity 的事实 envelope。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentEvent {
    pub(super) agent_id: AgentId,
    pub(super) turn_id: AgentTurnId,
    pub(super) target: RuntimeTarget,
    pub(super) kind: AgentEventKind,
}

/// `AgentEventKind` 只包含 Agent turn/progress/tool/permission/terminal 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentEventKind {
    SystemMessage {
        message: String,
    },
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: usize,
    },
    InputTokenEstimate {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    AssistantDelta {
        content: String,
    },
    ReasoningDelta {
        content: String,
    },
    ToolActivityStarted {
        activity: RuntimeToolActivity,
    },
    ToolActivityUpdated {
        update: RuntimeToolActivityUpdate,
    },
    TerminalUpdated {
        snapshot: RuntimeTerminalSnapshot,
    },
    PermissionRequested {
        request: RuntimePermissionRequest,
    },
    /// 环境探测失败仍按旧行为继续用空 injection 启动 turn，因此不是 terminal fact。
    PreparationWarning {
        message: String,
    },
    TurnFinished {
        response: ConversationResponse,
        metrics: Option<RuntimeRequestMetrics>,
        context_usage: Option<ContextWindowUsage>,
    },
    TurnFailed {
        message: String,
    },
    TurnInterrupted,
}

impl AgentEventKind {
    pub(super) const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TurnFinished { .. } | Self::TurnFailed { .. } | Self::TurnInterrupted
        )
    }
}

/// `AgentRuntimeError` 区分 command admission 与 lifecycle 失败。
#[derive(Debug, thiserror::Error)]
pub(super) enum AgentRuntimeError {
    #[error("Agent runtime is disposed")]
    Disposed,
    #[error("Unknown agent handle")]
    UnknownAgent,
    #[error("Conversation request is already running")]
    Busy,
    #[error("{0}")]
    CommandRejected(String),
    #[error("Agent runtime shutdown failed: {0}")]
    Shutdown(String),
}

/// `AgentRuntime` 是 Agent loop 的最小内部 seam。
pub(super) trait AgentRuntime {
    fn dispatch(&mut self, command: AgentCommand)
    -> Result<AgentCommandReceipt, AgentRuntimeError>;

    /// 非阻塞、FIFO 地取出当前已就绪的全部 Agent facts。
    fn drain_events(&mut self) -> Vec<AgentEvent>;

    /// 幂等撤销 runtime 拥有的全部副作用并等待 producer quiescence。
    fn shutdown(&mut self) -> Result<(), AgentRuntimeError>;
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
