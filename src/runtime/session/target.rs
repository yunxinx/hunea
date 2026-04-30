/// `RuntimeTarget` 标识一个可由 TUI 驱动的交互式 runtime。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuntimeTarget {
    AcpAgent { agent_id: String },
    NativeAgent(NativeRuntimeTarget),
}

impl RuntimeTarget {
    /// `acp_agent` 创建 ACP agent runtime 目标。
    pub fn acp_agent(agent_id: impl Into<String>) -> Self {
        Self::AcpAgent {
            agent_id: agent_id.into(),
        }
    }

    /// `native_agent` 创建原生 agent runtime 目标。
    pub fn native_agent(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self::NativeAgent(NativeRuntimeTarget::new(provider_id, model_id))
    }

    /// `display_label` 返回适合状态行使用的短标签。
    pub fn display_label(&self) -> &str {
        match self {
            Self::AcpAgent { agent_id } => agent_id,
            Self::NativeAgent(target) => &target.model_id,
        }
    }
}

/// `NativeRuntimeTarget` 保存 native provider/model 组合。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativeRuntimeTarget {
    pub provider_id: String,
    pub model_id: String,
}

impl NativeRuntimeTarget {
    /// `new` 创建 native runtime 目标。
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }
}
