/// `RuntimeTarget` 标识一个可由 TUI 驱动的交互式 runtime。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuntimeTarget {
    Provider(ProviderTarget),
}

impl RuntimeTarget {
    /// `provider` 创建 provider/model 组合对应的 runtime 目标。
    pub fn provider(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self::Provider(ProviderTarget::new(provider_id, model_id))
    }

    /// `display_label` 返回适合状态行使用的短标签。
    pub fn display_label(&self) -> &str {
        match self {
            Self::Provider(target) => &target.model_id,
        }
    }
}

/// `ProviderTarget` 保存 provider/model 组合。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderTarget {
    pub provider_id: String,
    pub model_id: String,
}

impl ProviderTarget {
    /// `new` 创建 provider runtime 目标。
    pub fn new(provider_id: impl Into<String>, model_id: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        }
    }
}
