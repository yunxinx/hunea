/// `RuntimeIdentity` 描述 runtime 对 TUI 暴露的显示身份。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIdentity {
    pub label: String,
    pub source_label: Option<String>,
    pub version: Option<String>,
}

impl RuntimeIdentity {
    /// `new` 使用主显示名创建 runtime identity。
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            source_label: None,
            version: None,
        }
    }

    /// `with_source_label` 附加来源标签，例如 provider id 或 ACP 配置 key。
    pub fn with_source_label(mut self, source_label: impl Into<String>) -> Self {
        self.source_label = Some(source_label.into());
        self
    }

    /// `with_version` 附加 runtime/agent 版本号。
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
}
