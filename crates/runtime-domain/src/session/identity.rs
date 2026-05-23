/// `RuntimeIdentity` 描述 runtime 对 TUI 暴露的显示身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub label: String,
    pub source_label: Option<String>,
    pub version: Option<String>,
    pub has_agent_info: bool,
    pub agent_capabilities: Option<RuntimeAgentCapabilities>,
}

impl RuntimeIdentity {
    /// `new` 使用主显示名创建 runtime identity。
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            source_label: None,
            version: None,
            has_agent_info: true,
            agent_capabilities: None,
        }
    }

    /// `with_source_label` 附加来源标签，例如 provider id。
    pub fn with_source_label(mut self, source_label: impl Into<String>) -> Self {
        self.source_label = Some(source_label.into());
        self
    }

    /// `with_version` 附加 runtime/agent 版本号。
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// `with_agent_capabilities` 附加 runtime 上报的能力摘要。
    pub fn with_agent_capabilities(mut self, capabilities: RuntimeAgentCapabilities) -> Self {
        self.agent_capabilities = Some(capabilities);
        self
    }

    /// `without_agent_info` 表示显示名来自配置或 fallback，而不是 runtime 自报信息。
    pub fn without_agent_info(mut self) -> Self {
        self.has_agent_info = false;
        self
    }
}

/// `RuntimeAgentCapabilities` 是 runtime agent 可展示/可消费能力摘要。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAgentCapabilities {
    pub load_session: bool,
    pub prompt_capabilities: RuntimePromptCapabilities,
}

/// `RuntimePromptCapabilities` 描述 runtime 可接受的 prompt block 类型。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimePromptCapabilities {
    pub image: bool,
    pub audio: bool,
    pub embedded_context: bool,
}
