//! Framework-neutral Agent runtime contract.
//!
//! This module contains only identifiers, command/event DTOs and the minimal lifecycle trait. Host
//! capabilities, provider implementations and worker ownership stay in the composition root.

use std::fmt;

use thiserror::Error;

use crate::{
    context_budget::ContextWindowUsage,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimePermissionRequest,
        RuntimeRequestMetrics, RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
        RuntimeToolActivityUpdate, TranscriptCustomPromptBinding, TranscriptSkillBinding,
        TranscriptUserAttachment, TranscriptUserMessage,
    },
};

/// `AgentId` 标识一个由 runtime host 管理的 Agent handle。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentId(u64);

impl fmt::Debug for AgentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentId")
    }
}

impl AgentId {
    /// 主 Agent 的稳定 handle。
    pub const MAIN: Self = Self(1);

    /// 从 host 分配的数值创建 Agent handle。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回跨进程协议使用的稳定数值 identity。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// `AgentTurnId` 标识一次 Agent turn，避免事件依赖“唯一活跃 worker”的隐式假设。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentTurnId(u64);

impl fmt::Debug for AgentTurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentTurnId")
    }
}

impl AgentTurnId {
    /// 从 host 分配的数值创建 turn identity。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// 返回跨进程协议使用的稳定数值 identity。
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// `AgentUserDelivery` 只保存用户实际提交并应在 transcript 中显示的内容。
#[derive(Clone, PartialEq, Eq)]
struct AgentUserDelivery {
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
struct AgentTurnControls {
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
/// provider target envelope 只保存 provider/model identity 与 provider-visible message，不携带
/// credential、endpoint 或 instruction body。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentTurnRequest {
    delivery: AgentUserDelivery,
    controls: AgentTurnControls,
    native_request: ConversationTurnRequest,
}

impl AgentTurnRequest {
    /// 从跨层的 conversation request 创建结构化 Agent request。
    pub fn from_conversation_request(request: ConversationTurnRequest) -> Self {
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

    /// 返回该 turn 对应的 runtime target。
    pub fn target(&self) -> RuntimeTarget {
        self.native_request.target()
    }

    /// 返回用于活动摘要的 model label。
    pub fn activity_label(&self) -> &str {
        self.native_request.model_id()
    }

    /// 返回当前 adapter 所需的 conversation request 只读视图。
    pub fn conversation_request(&self) -> &ConversationTurnRequest {
        &self.native_request
    }

    /// 将 Agent request 还原为当前 adapter 所消费的领域 request 与 transcript message。
    pub fn into_parts(self) -> (ConversationTurnRequest, TranscriptUserMessage) {
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
            .field("has_target", &true)
            .field("delivery", &self.delivery)
            .field("controls", &self.controls)
            .finish()
    }
}

/// `AgentCommand` 只描述 Agent loop 行为；session tree、prompt editor、model refresh 与 context
/// budget 继续由 host capability 处理。
pub enum AgentCommand {
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
            Self::SubmitTurn { request, .. } => f
                .debug_struct("SubmitTurn")
                .field("has_agent_id", &true)
                .field("has_turn_id", &true)
                .field("request", request)
                .finish(),
            Self::Interrupt { target, .. } => f
                .debug_struct("Interrupt")
                .field("has_agent_id", &true)
                .field("has_target", &target.is_some())
                .finish(),
            Self::RespondPermission {
                target,
                request_id,
                option_id,
                ..
            } => f
                .debug_struct("RespondPermission")
                .field("has_agent_id", &true)
                .field("has_target", &target.is_some())
                .field("has_request_id", &!request_id.is_empty())
                .field("has_option", &option_id.is_some())
                .finish(),
        }
    }
}

/// `AgentCommandReceipt` 只确认命令已原子准入；异步执行结果通过 `AgentEvent` 交付。
#[derive(Clone, PartialEq, Eq)]
pub enum AgentCommandReceipt {
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

impl fmt::Debug for AgentCommandReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted => formatter.write_str("Accepted"),
            Self::TurnStarted { .. } => formatter.write_str("TurnStarted"),
            Self::Interrupted { target } => formatter
                .debug_struct("Interrupted")
                .field("has_target", &target.is_some())
                .finish(),
        }
    }
}

/// `AgentEvent` 是带显式 Agent/turn identity 的事实 envelope。
#[derive(Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub agent_id: AgentId,
    pub turn_id: AgentTurnId,
    pub target: RuntimeTarget,
    pub kind: AgentEventKind,
}

impl fmt::Debug for AgentEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentEvent")
            .field("has_agent_id", &(self.agent_id.get() != 0))
            .field("has_turn_id", &(self.turn_id.get() != 0))
            .field("kind", &self.kind)
            .finish()
    }
}

/// `AgentEventKind` 只包含 Agent turn/progress/tool/permission/terminal 事实。
#[derive(Clone, PartialEq, Eq)]
pub enum AgentEventKind {
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

impl fmt::Debug for AgentEventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::SystemMessage { .. } => "SystemMessage",
            Self::Retrying { .. } => "Retrying",
            Self::OutputTokenEstimate { .. } => "OutputTokenEstimate",
            Self::InputTokenEstimate { .. } => "InputTokenEstimate",
            Self::Thinking { .. } => "Thinking",
            Self::AssistantDelta { .. } => "AssistantDelta",
            Self::ReasoningDelta { .. } => "ReasoningDelta",
            Self::ToolActivityStarted { .. } => "ToolActivityStarted",
            Self::ToolActivityUpdated { .. } => "ToolActivityUpdated",
            Self::TerminalUpdated { .. } => "TerminalUpdated",
            Self::PermissionRequested { .. } => "PermissionRequested",
            Self::PreparationWarning { .. } => "PreparationWarning",
            Self::TurnFinished { .. } => "TurnFinished",
            Self::TurnFailed { .. } => "TurnFailed",
            Self::TurnInterrupted => "TurnInterrupted",
        };
        formatter.write_str(name)
    }
}

impl AgentEventKind {
    /// 判断 event 是否结束当前 turn。
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::TurnFinished { .. } | Self::TurnFailed { .. } | Self::TurnInterrupted
        )
    }
}

/// `AgentRuntimeError` 区分 command admission 与 lifecycle 失败。
#[derive(Debug, Error)]
pub enum AgentRuntimeError {
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

/// Agent loop 的最小 framework-neutral seam。
pub trait AgentRuntime: Send {
    /// 同步接收一个 Agent command；异步事实通过 [`AgentRuntime::drain_events`] 交付。
    fn dispatch(&mut self, command: AgentCommand)
    -> Result<AgentCommandReceipt, AgentRuntimeError>;

    /// 非阻塞、FIFO 地取出当前已就绪的全部 Agent facts。
    fn drain_events(&mut self) -> Vec<AgentEvent>;

    /// 幂等撤销 runtime 拥有的全部副作用并等待 producer quiescence。
    fn shutdown(&mut self) -> Result<(), AgentRuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::{AgentCommand, AgentEventKind, AgentId, AgentTurnId, AgentTurnRequest};
    use crate::session::{
        ConversationTurnRequest, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget, TranscriptUserMessage,
    };

    #[test]
    fn request_debug_keeps_delivery_and_control_bodies_out_of_diagnostics() {
        let request = AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_source_message(
                "local",
                "qwen3",
                TranscriptUserMessage {
                    content: "visible delivery".to_string(),
                    attachments: Vec::new(),
                    skill_bindings: Vec::new(),
                    custom_prompt_bindings: Vec::new(),
                },
            ),
        );
        let debug = format!("{request:?}");

        assert!(debug.contains("content_chars"));
        assert!(!debug.contains("visible delivery"));
    }

    #[test]
    fn agent_event_identity_and_terminal_classification_are_explicit() {
        let event = super::AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(9),
            target: super::RuntimeTarget::provider("local", "qwen3"),
            kind: AgentEventKind::TurnInterrupted,
        };

        assert_eq!(event.agent_id, AgentId::MAIN);
        assert_eq!(event.turn_id, AgentTurnId::new(9));
        assert!(event.kind.is_terminal());
    }

    #[test]
    fn agent_command_and_event_debug_omit_delivery_and_correlation_bodies() {
        let command = AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(RuntimeTarget::provider("secret-provider", "secret-model")),
            request_id: "secret-permission-id".to_string(),
            option_id: Some("secret-option-id".to_string()),
        };
        let event = super::AgentEvent {
            agent_id: AgentId::new(41),
            turn_id: AgentTurnId::new(42),
            target: RuntimeTarget::provider("secret-provider", "secret-model"),
            kind: AgentEventKind::PermissionRequested {
                request: RuntimePermissionRequest::new(
                    "secret-permission-id",
                    Some("secret permission body".to_string()),
                    vec![RuntimePermissionOption::new(
                        "secret-option-id",
                        "secret option body",
                        RuntimePermissionOptionKind::AllowOnce,
                    )],
                ),
            },
        };
        let debug = format!("{command:?}\n{event:?}");

        for forbidden in [
            "secret-permission-id",
            "secret-option-id",
            "secret permission body",
            "secret option body",
            "secret-provider",
            "secret-model",
            "41",
            "42",
        ] {
            assert!(!debug.contains(forbidden), "leaked {forbidden}");
        }
    }
}
