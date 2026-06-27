//! Context budget snapshot for the next prepared provider turn.

use provider_protocol::{ConversationItem, Role};

use crate::token_count::estimate_text_tokens;

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
    pub stack_order: u16,
    pub estimated_tokens: usize,
    pub label: String,
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
    Relative { used: u32 },
    Absolute { limit: u32, used: u32, percent: f32 },
}

/// Estimated token breakdown for one prepared turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextBudgetSnapshot {
    pub model_id: String,
    pub segments: Vec<ContextSegment>,
    pub total_estimated_tokens: usize,
    pub context_limit: Option<u32>,
    pub display: ContextLimitDisplay,
}

/// Builds a snapshot from the same ordered `items` as `prepare_turn` plus optional tool schema text.
///
/// **Double-counting rule**: `items` must be exactly `PreparedConversationRequest::items()` —
/// one `ConversationItem` yields exactly one segment (in list order). The pending user turn is
/// only the final user message inside `items`; there is no separate pending segment.
/// `tool_definitions_text` is optional and becomes at most one additional segment after all items.
pub fn build_context_budget_snapshot(
    model_id: &str,
    items: &[ConversationItem],
    tool_definitions_text: Option<&str>,
    context_limit: Option<u32>,
) -> ContextBudgetSnapshot {
    let mut segments = Vec::new();
    let mut stack_order: u16 = 0;

    for item in items {
        let (kind, label) = segment_kind_and_label(item);
        let text = estimation_text_for_item(item);
        let estimated_tokens = estimate_text_tokens(model_id, &text);
        segments.push(ContextSegment {
            kind,
            stack_order,
            estimated_tokens,
            label,
        });
        stack_order = stack_order.saturating_add(1);
    }

    if let Some(tool_text) = tool_definitions_text.filter(|t| !t.is_empty()) {
        let estimated_tokens = estimate_text_tokens(model_id, tool_text);
        segments.push(ContextSegment {
            kind: SegmentKind::ToolDefinitions,
            stack_order,
            estimated_tokens,
            label: SegmentKind::ToolDefinitions.default_label().to_string(),
        });
    }

    let total_estimated_tokens: usize = segments.iter().map(|s| s.estimated_tokens).sum();
    let used = u32::try_from(total_estimated_tokens).unwrap_or(u32::MAX);
    let display = match context_limit {
        Some(limit) if limit > 0 => ContextLimitDisplay::Absolute {
            limit,
            used,
            percent: (used as f32 / limit as f32) * 100.0,
        },
        _ => ContextLimitDisplay::Relative { used },
    };

    ContextBudgetSnapshot {
        model_id: model_id.to_string(),
        segments,
        total_estimated_tokens,
        context_limit,
        display,
    }
}

fn segment_kind_and_label(item: &ConversationItem) -> (SegmentKind, String) {
    match item {
        ConversationItem::Message { role, .. } => {
            let kind = match role {
                Role::System => SegmentKind::System,
                Role::User => SegmentKind::UserMessage,
                Role::Assistant => SegmentKind::AssistantMessage,
            };
            (kind, kind.default_label().to_string())
        }
        ConversationItem::ToolResult { .. } => (
            SegmentKind::ToolResult,
            SegmentKind::ToolResult.default_label().to_string(),
        ),
        ConversationItem::Reasoning { .. } => (
            SegmentKind::Reasoning,
            SegmentKind::Reasoning.default_label().to_string(),
        ),
    }
}

fn estimation_text_for_item(item: &ConversationItem) -> String {
    match item {
        ConversationItem::Message { .. } | ConversationItem::ToolResult { .. } => {
            item.text_content()
        }
        ConversationItem::Reasoning { content, .. } => content.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_protocol::{ContentBlock, ToolCall};

    fn turn_items(items: Vec<ConversationItem>) -> ContextBudgetSnapshot {
        build_context_budget_snapshot("gpt-4o", &items, None, None)
    }

    #[test]
    fn three_message_roles_yield_three_segments_in_order() {
        let items = vec![
            ConversationItem::text(Role::System, "sys"),
            ConversationItem::text(Role::User, "u1"),
            ConversationItem::text(Role::Assistant, "a1"),
            ConversationItem::text(Role::User, "u2"),
        ];
        let snapshot = turn_items(items);
        assert_eq!(snapshot.segments.len(), 4);
        assert_eq!(
            snapshot.segments.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![
                SegmentKind::System,
                SegmentKind::UserMessage,
                SegmentKind::AssistantMessage,
                SegmentKind::UserMessage,
            ]
        );
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|s| s.stack_order)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn user_and_assistant_never_merge_into_one_segment() {
        let snapshot = turn_items(vec![
            ConversationItem::text(Role::User, "one"),
            ConversationItem::text(Role::Assistant, "two"),
        ]);
        assert_eq!(snapshot.segments.len(), 2);
        assert_ne!(snapshot.segments[0].kind, snapshot.segments[1].kind);
    }

    #[test]
    fn pending_user_only_appears_once_in_items_list() {
        let items = vec![
            ConversationItem::text(Role::User, "history"),
            ConversationItem::text(Role::Assistant, "reply"),
            ConversationItem::text(Role::User, "pending"),
        ];
        let snapshot = turn_items(items);
        let user_segments: Vec<_> = snapshot
            .segments
            .iter()
            .filter(|s| s.kind == SegmentKind::UserMessage)
            .collect();
        assert_eq!(user_segments.len(), 2);
        assert_eq!(snapshot.segments.len(), 3);
    }

    #[test]
    fn tool_definitions_segment_when_schema_non_empty() {
        let snapshot = build_context_budget_snapshot(
            "gpt-4o",
            &[ConversationItem::text(Role::User, "hi")],
            Some(r#"{"name":"read"}"#),
            None,
        );
        assert!(
            snapshot
                .segments
                .iter()
                .any(|s| s.kind == SegmentKind::ToolDefinitions && s.estimated_tokens > 0)
        );
    }

    #[test]
    fn relative_display_when_no_context_limit() {
        let snapshot = turn_items(vec![ConversationItem::text(Role::User, "hello world")]);
        assert!(matches!(
            snapshot.display,
            ContextLimitDisplay::Relative { .. }
        ));
        let total: usize = snapshot.segments.iter().map(|s| s.estimated_tokens).sum();
        let sum_pct: f32 = snapshot
            .segments
            .iter()
            .map(|s| ContextSegment::share_of_segments_percent(total, s.estimated_tokens))
            .sum();
        assert!((sum_pct - 100.0).abs() < 0.01);
    }

    #[test]
    fn absolute_display_when_context_limit_set() {
        let snapshot = build_context_budget_snapshot(
            "gpt-4o",
            &[ConversationItem::text(Role::User, "hello")],
            None,
            Some(128_000),
        );
        match snapshot.display {
            ContextLimitDisplay::Absolute {
                limit,
                used,
                percent,
            } => {
                assert_eq!(limit, 128_000);
                assert_eq!(used as usize, snapshot.total_estimated_tokens);
                assert!((percent - (used as f32 / 128_000.0 * 100.0)).abs() < 0.01);
            }
            ContextLimitDisplay::Relative { .. } => panic!("expected absolute display"),
        }
    }

    #[test]
    fn tool_result_and_assistant_with_tool_calls_are_distinct_segments() {
        let assistant = ConversationItem::assistant_with_tool_calls(
            "call it".to_string(),
            vec![ToolCall {
                call_id: "c1".to_string(),
                name: "read".to_string(),
                arguments: "{}".to_string(),
            }],
        );
        let snapshot = turn_items(vec![
            assistant,
            ConversationItem::tool_result("c1", vec![ContentBlock::Text("ok".into())], false),
        ]);
        assert_eq!(snapshot.segments.len(), 2);
        assert_eq!(snapshot.segments[0].kind, SegmentKind::AssistantMessage);
        assert_eq!(snapshot.segments[1].kind, SegmentKind::ToolResult);
    }
}
