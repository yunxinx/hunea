use super::{NativeAgentTurnRequest, RuntimeTarget};

/// `RuntimeCommand` 描述 TUI 向交互式 runtime 发出的统一命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    SubmitNativeAgent {
        target: RuntimeTarget,
        request: NativeAgentTurnRequest,
    },
    TruncateNativeAgentSession {
        retained_user_turns: usize,
    },
    Interrupt {
        target: Option<RuntimeTarget>,
    },
    RespondPermission {
        target: Option<RuntimeTarget>,
        request_id: String,
        option_id: Option<String>,
    },
    Reset,
}

impl RuntimeCommand {
    /// `submit_native_agent` 创建 native agent 提交命令。
    pub fn submit_native_agent(request: NativeAgentTurnRequest) -> Self {
        Self::SubmitNativeAgent {
            target: request.target(),
            request,
        }
    }

    /// `truncate_native_agent_session` 创建 native agent 历史截断命令。
    pub fn truncate_native_agent_session(retained_user_turns: usize) -> Self {
        Self::TruncateNativeAgentSession {
            retained_user_turns,
        }
    }

    /// `interrupt_current` 创建打断当前 turn 的命令。
    pub fn interrupt_current() -> Self {
        Self::Interrupt { target: None }
    }

    /// `respond_permission` 创建权限确认响应命令。
    pub fn respond_permission(
        target: RuntimeTarget,
        request_id: impl Into<String>,
        option_id: Option<String>,
    ) -> Self {
        Self::RespondPermission {
            target: Some(target),
            request_id: request_id.into(),
            option_id,
        }
    }

    /// `target` 返回命令关联的 runtime 目标。
    pub fn target(&self) -> Option<&RuntimeTarget> {
        match self {
            Self::SubmitNativeAgent { target, .. }
            | Self::Interrupt {
                target: Some(target),
            }
            | Self::RespondPermission {
                target: Some(target),
                ..
            } => Some(target),
            Self::Interrupt { target: None }
            | Self::RespondPermission { target: None, .. }
            | Self::TruncateNativeAgentSession { .. }
            | Self::Reset => None,
        }
    }
}

/// `RuntimeCommandReceipt` 描述 runtime coordinator 接受命令后的同步结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommandReceipt {
    Accepted,
    NativeAgentStarted { activity_label: String },
    Interrupted { target: Option<RuntimeTarget> },
}
