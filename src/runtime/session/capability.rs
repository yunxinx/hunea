/// `RuntimeCapability` 描述 TUI 可据此启用的 runtime 能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCapability {
    pub supports_tools: bool,
    pub supports_permissions: bool,
    pub supports_model_config: bool,
}

impl RuntimeCapability {
    /// `native_chat` 返回普通 native chat 的能力快照。
    pub const fn native_chat() -> Self {
        Self {
            supports_tools: false,
            supports_permissions: false,
            supports_model_config: false,
        }
    }

    /// `agent` 返回具备工具与权限通道的 agent runtime 能力快照。
    pub const fn agent() -> Self {
        Self {
            supports_tools: true,
            supports_permissions: true,
            supports_model_config: false,
        }
    }

    /// `acp` 返回当前 ACP 会话对 TUI 暴露的能力快照。
    pub const fn acp() -> Self {
        Self {
            supports_tools: false,
            supports_permissions: true,
            supports_model_config: true,
        }
    }
}
