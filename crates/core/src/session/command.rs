use crate::acp::AcpPromptRequest;

use super::{NativeAgentTurnRequest, RuntimeTarget};

/// `RuntimeCommand` 描述 TUI 向交互式 runtime 发出的统一命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommand {
    Start {
        target: RuntimeTarget,
    },
    SubmitPrompt {
        target: RuntimeTarget,
        prompt: String,
    },
    SubmitAcpPrompt {
        target: RuntimeTarget,
        prompt: AcpPromptRequest,
    },
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
    SetConfigOption {
        target: Option<RuntimeTarget>,
        config_id: Option<String>,
        value: String,
    },
    StopBackgroundTerminals {
        target: Option<RuntimeTarget>,
    },
    Reset,
}

impl RuntimeCommand {
    /// `start` 创建启动 runtime 的命令。
    pub fn start(target: RuntimeTarget) -> Self {
        Self::Start { target }
    }

    /// `submit_prompt` 创建提交用户 prompt 的命令。
    pub fn submit_prompt(target: RuntimeTarget, prompt: impl Into<String>) -> Self {
        Self::SubmitPrompt {
            target,
            prompt: prompt.into(),
        }
    }

    /// `submit_acp_prompt` 创建 ACP prompt 提交命令。
    pub fn submit_acp_prompt(prompt: AcpPromptRequest) -> Self {
        Self::SubmitAcpPrompt {
            target: RuntimeTarget::acp_agent(prompt.agent_id.clone()),
            prompt,
        }
    }

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

    /// `set_config_option` 创建 runtime 配置项更新命令。
    pub fn set_config_option(
        target: RuntimeTarget,
        config_id: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::SetConfigOption {
            target: Some(target),
            config_id: Some(config_id.into()),
            value: value.into(),
        }
    }

    /// `stop_background_terminals` 创建停止后台 terminal 的命令。
    pub fn stop_background_terminals(target: Option<RuntimeTarget>) -> Self {
        Self::StopBackgroundTerminals { target }
    }

    /// `target` 返回命令关联的 runtime 目标。
    pub fn target(&self) -> Option<&RuntimeTarget> {
        match self {
            Self::Start { target }
            | Self::SubmitPrompt { target, .. }
            | Self::SubmitAcpPrompt { target, .. }
            | Self::SubmitNativeAgent { target, .. }
            | Self::Interrupt {
                target: Some(target),
            }
            | Self::RespondPermission {
                target: Some(target),
                ..
            }
            | Self::SetConfigOption {
                target: Some(target),
                ..
            }
            | Self::StopBackgroundTerminals {
                target: Some(target),
            } => Some(target),
            Self::Interrupt { target: None }
            | Self::RespondPermission { target: None, .. }
            | Self::SetConfigOption { target: None, .. }
            | Self::StopBackgroundTerminals { target: None }
            | Self::TruncateNativeAgentSession { .. }
            | Self::Reset => None,
        }
    }
}

/// `RuntimeCommandReceipt` 描述 runtime coordinator 接受命令后的同步结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeCommandReceipt {
    Accepted,
    AcpSessionStarted { default_model: Option<String> },
    NativeAgentStarted { activity_label: String },
    Interrupted { target: Option<RuntimeTarget> },
}
