/// `RuntimeCapability` 描述 TUI 可据此启用的 runtime 能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCapability {
    pub supports_tools: bool,
    pub supports_permissions: bool,
    pub supports_model_config: bool,
}

impl RuntimeCapability {
    /// `conversation` 返回当前对话运行时能力快照。
    pub const fn conversation() -> Self {
        Self {
            supports_tools: true,
            supports_permissions: true,
            supports_model_config: false,
        }
    }
}
