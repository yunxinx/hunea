use serde::{Deserialize, Serialize};

/// `PromptAssemblyScope` 表示 prompt assembly 配置的生效范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptAssemblyScope {
    Global,
    Project,
}

impl PromptAssemblyScope {
    /// `as_stored_value` 返回稳定的持久化值。
    pub const fn as_stored_value(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    /// `from_stored_value` 从稳定持久化值恢复 scope。
    #[must_use]
    pub fn from_stored_value(value: &str) -> Option<Self> {
        match value {
            "global" => Some(Self::Global),
            "project" => Some(Self::Project),
            _ => None,
        }
    }
}
