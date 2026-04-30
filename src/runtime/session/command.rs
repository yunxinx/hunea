use super::RuntimeTarget;

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
        config_id: String,
        value: String,
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
            config_id: config_id.into(),
            value: value.into(),
        }
    }

    /// `target` 返回命令关联的 runtime 目标。
    pub fn target(&self) -> Option<&RuntimeTarget> {
        match self {
            Self::Start { target }
            | Self::SubmitPrompt { target, .. }
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
            } => Some(target),
            Self::Interrupt { target: None }
            | Self::RespondPermission { target: None, .. }
            | Self::SetConfigOption { target: None, .. }
            | Self::Reset => None,
        }
    }
}
