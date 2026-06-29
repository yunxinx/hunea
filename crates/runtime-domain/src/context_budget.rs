//! Context budget snapshot for the next prepared provider turn.

/// Extensible segment kind for context budget breakdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    System,
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
    pub stack_order: usize,
    pub estimated_tokens: usize,
}

impl ContextSegment {
    /// Share of total segment tokens in `[0, 100]` when total > 0.
    pub fn share_of_segments_percent(total_tokens: usize, segment_tokens: usize) -> f32 {
        if total_tokens == 0 {
            return 0.0;
        }
        (segment_tokens as f32 / total_tokens as f32) * 100.0
    }
}

/// How context limit is shown relative to estimated usage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContextLimitDisplay {
    Absolute { limit: u32, used: u32, percent: f32 },
}

/// Estimated token breakdown for one prepared turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudgetSnapshot {
    pub model_id: String,
    pub segments: Vec<ContextSegment>,
    pub total_estimated_tokens: usize,
    pub context_limit: u32,
    pub display: ContextLimitDisplay,
}

/// `context_limit_display` 根据总 token 数与展示上限构造绝对展示模式。
pub fn context_limit_display(
    total_estimated_tokens: usize,
    context_limit: u32,
) -> ContextLimitDisplay {
    let used = u32::try_from(total_estimated_tokens).unwrap_or(u32::MAX);
    ContextLimitDisplay::Absolute {
        limit: context_limit,
        used,
        percent: (used as f32 / context_limit as f32) * 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_display_uses_documented_fallback_limit() {
        match context_limit_display(12_345, 256_000) {
            ContextLimitDisplay::Absolute {
                limit,
                used,
                percent,
            } => {
                assert_eq!(limit, 256_000);
                assert_eq!(used, 12_345);
                assert!((percent - (used as f32 / 256_000.0 * 100.0)).abs() < 0.01);
            }
        }
    }

    #[test]
    fn absolute_display_when_context_limit_set() {
        match context_limit_display(32_000, 128_000) {
            ContextLimitDisplay::Absolute {
                limit,
                used,
                percent,
            } => {
                assert_eq!(limit, 128_000);
                assert_eq!(used, 32_000);
                assert!((percent - (used as f32 / 128_000.0 * 100.0)).abs() < 0.01);
            }
        }
    }
}
