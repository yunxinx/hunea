//! Context budget snapshot for the next prepared provider turn.

use std::{fmt, num::NonZeroUsize};

/// Extensible segment kind for context budget breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    System,
    SkillDiscovery,
    UserMessage,
    AssistantMessage,
    ToolResult,
    Reasoning,
    ToolDefinitions,
}

impl SegmentKind {
    /// Stable label for legend rows.
    pub const fn default_label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::SkillDiscovery => "skill_discovery",
            Self::UserMessage => "user",
            Self::AssistantMessage => "assistant",
            Self::ToolResult => "tool_result",
            Self::Reasoning => "reasoning",
            Self::ToolDefinitions => "tools",
        }
    }
}

/// One measurable fragment in the context budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSegment {
    pub kind: SegmentKind,
    pub estimated_tokens: usize,
}

/// `ContextTokenLimit` 表示一个严格大于 0 的 context token 上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContextTokenLimit(NonZeroUsize);

impl ContextTokenLimit {
    /// `get` 返回原始的正整数 token 上限。
    pub const fn get(self) -> usize {
        self.0.get()
    }

    /// `new` 从原始整数构造非零上限。
    pub const fn new(value: usize) -> Option<Self> {
        match NonZeroUsize::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl TryFrom<usize> for ContextTokenLimit {
    type Error = ContextTokenLimitError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(ContextTokenLimitError)
    }
}

impl From<ContextTokenLimit> for usize {
    fn from(value: ContextTokenLimit) -> Self {
        value.get()
    }
}

/// `ContextTokenLimitError` 表示 context token 上限不是正整数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextTokenLimitError;

impl fmt::Display for ContextTokenLimitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context token limit must be greater than zero")
    }
}

impl std::error::Error for ContextTokenLimitError {}

/// Absolute usage summary for one context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindowUsage {
    pub limit: ContextTokenLimit,
    /// `used` 保留完整估算值，避免大上下文在 UI 边界静默饱和。
    pub used: usize,
}

/// Estimated token breakdown for one prepared turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSnapshot {
    pub model_id: String,
    pub segments: Vec<ContextSegment>,
    pub total_estimated_tokens: usize,
    pub usage: ContextWindowUsage,
}

/// `share_of_total_percent` 计算部分相对总量的百分比。
pub fn share_of_total_percent(part: usize, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }

    (part as f32 / total as f32) * 100.0
}

/// `context_window_usage` 根据总 token 数与展示上限构造绝对用量摘要。
pub fn context_window_usage(
    total_estimated_tokens: usize,
    context_limit: ContextTokenLimit,
) -> ContextWindowUsage {
    ContextWindowUsage {
        limit: context_limit,
        used: total_estimated_tokens,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_token_limit_rejects_zero() {
        assert!(
            ContextTokenLimit::try_from(0).is_err(),
            "zero should stay invalid at the type boundary"
        );
    }

    #[test]
    fn context_token_limit_preserves_usize_width() {
        if usize::BITS <= u32::BITS {
            return;
        }

        let large_limit = usize::try_from(u32::MAX).expect("u32::MAX should fit in usize") + 1;

        assert_eq!(
            ContextTokenLimit::try_from(large_limit)
                .expect("usize-scale limit should be valid")
                .get(),
            large_limit
        );
    }

    #[test]
    fn context_window_usage_uses_documented_fallback_limit() {
        let usage = context_window_usage(
            12_345,
            ContextTokenLimit::try_from(256_000).expect("fixture limit should be valid"),
        );

        assert_eq!(usage.limit.get(), 256_000);
        assert_eq!(usage.used, 12_345);
    }

    #[test]
    fn context_window_usage_when_context_limit_set() {
        let usage = context_window_usage(
            32_000,
            ContextTokenLimit::try_from(128_000).expect("fixture limit should be valid"),
        );

        assert_eq!(usage.limit.get(), 128_000);
        assert_eq!(usage.used, 32_000);
    }

    #[test]
    fn context_window_usage_preserves_large_display_values() {
        let estimated_tokens = usize::try_from(u32::MAX).expect("u32::MAX should fit in usize") + 1;
        let usage = context_window_usage(
            estimated_tokens,
            ContextTokenLimit::try_from(256_000).expect("fixture limit should be valid"),
        );

        assert_eq!(usage.used, estimated_tokens);
    }

    #[test]
    fn share_of_total_percent_uses_zero_guard() {
        assert_eq!(share_of_total_percent(10, 0), 0.0);
    }

    #[test]
    fn share_of_total_percent_returns_expected_ratio() {
        assert!((share_of_total_percent(200, 500) - 40.0).abs() < f32::EPSILON);
    }
}
