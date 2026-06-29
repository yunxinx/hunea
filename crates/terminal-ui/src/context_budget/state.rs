use runtime_domain::context_budget::SegmentKind;
use runtime_domain::session::{
    ContextBudgetDisplayPayload, ContextBudgetLoadErrorPayload, ContextBudgetProjectionErrorKind,
    ContextBudgetSnapshotPayload, SessionLoadRequestId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextBudgetCategoryKind {
    SystemPrompt,
    ToolDefinitions,
    Messages,
    FreeSpace,
}

/// Legend row aggregated by stable context category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextBudgetLegendEntry {
    pub(crate) kind: ContextBudgetCategoryKind,
    pub(crate) label: String,
    pub(crate) estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ContextBudgetState {
    pub(crate) revision: usize,
    pub(crate) loading: bool,
    pub(crate) pending_request_id: Option<SessionLoadRequestId>,
    pub(crate) error: Option<ContextBudgetLoadErrorPayload>,
    pub(crate) snapshot: Option<ContextBudgetSnapshotPayload>,
}

impl Default for ContextBudgetState {
    fn default() -> Self {
        Self {
            revision: 1,
            loading: true,
            pending_request_id: None,
            error: None,
            snapshot: None,
        }
    }
}

impl ContextBudgetState {
    pub(crate) fn apply_snapshot(&mut self, payload: ContextBudgetSnapshotPayload) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.pending_request_id = None;
        self.error = None;
        self.snapshot = Some(payload);
    }

    pub(crate) fn set_error(&mut self, error: ContextBudgetLoadErrorPayload) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.pending_request_id = None;
        self.error = Some(error);
        self.snapshot = None;
    }
}

pub(crate) fn context_budget_error_message(error: &ContextBudgetLoadErrorPayload) -> String {
    match error {
        ContextBudgetLoadErrorPayload::UnknownProvider { provider_id } => {
            format!("Context budget could not find provider `{provider_id}`")
        }
        ContextBudgetLoadErrorPayload::UnsupportedProvider { provider_kind } => {
            format!("{provider_kind} cannot show context budget; use OpenAI/OpenAI-compatible.")
        }
        ContextBudgetLoadErrorPayload::ProjectionFailed { kind, .. } => match kind {
            ContextBudgetProjectionErrorKind::Internal => {
                "Failed to load context budget in the runtime.".to_string()
            }
            ContextBudgetProjectionErrorKind::Protocol => {
                "Failed to build context budget from provider payload.".to_string()
            }
            ContextBudgetProjectionErrorKind::Transport => {
                "Failed to build context budget because the provider is unavailable.".to_string()
            }
            ContextBudgetProjectionErrorKind::Provider => {
                "Failed to build context budget because the provider rejected the request."
                    .to_string()
            }
        },
    }
}

pub(crate) fn format_compact_tokens(tokens: usize) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let tenths = (tokens.saturating_mul(10).saturating_add(500)) / 1_000;
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if fraction == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{fraction}k")
    }
}

fn format_compact_percent(percent: f32) -> String {
    let tenths = (percent * 10.0).round() as i32;
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if fraction == 0 {
        format!("{whole}%")
    } else {
        format!("{whole}.{fraction}%")
    }
}

/// `context_usage_summary` 返回右侧图示首行使用的模型与上下文摘要。
pub(crate) fn context_usage_summary(
    model_id: &str,
    display: ContextBudgetDisplayPayload,
) -> String {
    match display {
        ContextBudgetDisplayPayload::Relative { used } => {
            format!(
                "{model_id} · {} tokens",
                format_compact_tokens(used as usize)
            )
        }
        ContextBudgetDisplayPayload::Absolute {
            limit,
            used,
            percent,
        } => {
            format!(
                "{model_id} · {}/{} tokens ({})",
                format_compact_tokens(used as usize),
                format_compact_tokens(limit as usize),
                format_compact_percent(percent),
            )
        }
    }
}

pub(crate) fn segment_share_percent(segment_tokens: usize, total_tokens: usize) -> f32 {
    if total_tokens == 0 {
        return 0.0;
    }
    (segment_tokens as f32 / total_tokens as f32) * 100.0
}

pub(crate) fn context_budget_category_display_rank(kind: ContextBudgetCategoryKind) -> usize {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => 0,
        ContextBudgetCategoryKind::ToolDefinitions => 1,
        ContextBudgetCategoryKind::Messages => 2,
        ContextBudgetCategoryKind::FreeSpace => 3,
    }
}

pub(crate) fn context_budget_category_label(kind: ContextBudgetCategoryKind) -> &'static str {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => "System prompt",
        ContextBudgetCategoryKind::ToolDefinitions => "Tool definitions",
        ContextBudgetCategoryKind::Messages => "Messages",
        ContextBudgetCategoryKind::FreeSpace => "Free space",
    }
}

pub(crate) fn context_budget_category_from_segment_kind(
    kind: SegmentKind,
) -> ContextBudgetCategoryKind {
    match kind {
        SegmentKind::System => ContextBudgetCategoryKind::SystemPrompt,
        SegmentKind::ToolDefinitions => ContextBudgetCategoryKind::ToolDefinitions,
        SegmentKind::UserMessage
        | SegmentKind::AssistantMessage
        | SegmentKind::ToolResult
        | SegmentKind::Reasoning => ContextBudgetCategoryKind::Messages,
    }
}

pub(crate) fn free_space_tokens(snapshot: &ContextBudgetSnapshotPayload) -> Option<usize> {
    match snapshot.display {
        ContextBudgetDisplayPayload::Absolute { limit, used, .. } => {
            Some(usize::try_from(limit.saturating_sub(used)).unwrap_or(usize::MAX))
        }
        ContextBudgetDisplayPayload::Relative { .. } => None,
    }
}

pub(crate) fn legend_share_total(snapshot: &ContextBudgetSnapshotPayload) -> usize {
    match snapshot.display {
        ContextBudgetDisplayPayload::Absolute { limit, .. } => {
            usize::try_from(limit).unwrap_or(usize::MAX)
        }
        ContextBudgetDisplayPayload::Relative { .. } => snapshot.total_estimated_tokens,
    }
}

pub(crate) fn aggregated_category_totals(
    snapshot: &ContextBudgetSnapshotPayload,
) -> [(ContextBudgetCategoryKind, usize); 3] {
    let mut totals = [0usize; 3];

    for segment in &snapshot.segments {
        let category = context_budget_category_from_segment_kind(segment.kind);
        let rank = context_budget_category_display_rank(category);
        if rank < totals.len() {
            totals[rank] = totals[rank].saturating_add(segment.estimated_tokens);
        }
    }

    [
        (ContextBudgetCategoryKind::SystemPrompt, totals[0]),
        (ContextBudgetCategoryKind::ToolDefinitions, totals[1]),
        (ContextBudgetCategoryKind::Messages, totals[2]),
    ]
}

pub(crate) fn build_legend_entries(
    snapshot: &ContextBudgetSnapshotPayload,
) -> Vec<ContextBudgetLegendEntry> {
    let mut entries = aggregated_category_totals(snapshot)
        .into_iter()
        .filter(|(_, estimated_tokens)| *estimated_tokens > 0)
        .map(|(kind, estimated_tokens)| ContextBudgetLegendEntry {
            kind,
            label: context_budget_category_label(kind).to_string(),
            estimated_tokens,
        })
        .collect::<Vec<_>>();

    if let Some(estimated_tokens) = free_space_tokens(snapshot).filter(|tokens| *tokens > 0) {
        entries.push(ContextBudgetLegendEntry {
            kind: ContextBudgetCategoryKind::FreeSpace,
            label: context_budget_category_label(ContextBudgetCategoryKind::FreeSpace).to_string(),
            estimated_tokens,
        });
    }

    entries.sort_by_key(|entry| context_budget_category_display_rank(entry.kind));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::session::ContextBudgetSegmentPayload;

    #[test]
    fn build_legend_entries_merges_duplicate_segment_kinds() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::UserMessage, 0, 120),
                segment(SegmentKind::AssistantMessage, 1, 200),
                segment(SegmentKind::UserMessage, 2, 80),
                segment(SegmentKind::Reasoning, 3, 40),
            ],
            total_estimated_tokens: 440,
            context_limit: Some(1_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 440,
                percent: 44.0,
            },
        };

        let entries = build_legend_entries(&snapshot);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, ContextBudgetCategoryKind::Messages);
        assert_eq!(entries[0].estimated_tokens, 440);
        assert_eq!(entries[1].kind, ContextBudgetCategoryKind::FreeSpace);
        assert_eq!(entries[1].estimated_tokens, 560);
    }

    #[test]
    fn aggregated_category_totals_group_messages_under_one_category() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::System, 0, 100),
                segment(SegmentKind::AssistantMessage, 1, 120),
                segment(SegmentKind::UserMessage, 2, 80),
                segment(SegmentKind::ToolResult, 3, 40),
                segment(SegmentKind::ToolDefinitions, 4, 20),
            ],
            total_estimated_tokens: 360,
            context_limit: Some(1_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 360,
                percent: 36.0,
            },
        };

        let totals = aggregated_category_totals(&snapshot);

        assert_eq!(totals[0], (ContextBudgetCategoryKind::SystemPrompt, 100));
        assert_eq!(totals[1], (ContextBudgetCategoryKind::ToolDefinitions, 20));
        assert_eq!(totals[2], (ContextBudgetCategoryKind::Messages, 240));
    }

    #[test]
    fn legend_share_total_uses_context_limit_when_available() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 400,
            context_limit: Some(1_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 400,
                percent: 40.0,
            },
        };

        assert_eq!(legend_share_total(&snapshot), 1_000);
    }

    #[test]
    fn free_space_tokens_uses_remaining_context_capacity() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 400,
            context_limit: Some(1_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 400,
                percent: 40.0,
            },
        };

        assert_eq!(free_space_tokens(&snapshot), Some(600));
    }

    #[test]
    fn segment_share_percent_uses_provided_total() {
        let percent = segment_share_percent(200, 500);
        assert!((percent - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_usage_summary_omits_total_percent() {
        let text = context_usage_summary(
            "gpt-4o",
            ContextBudgetDisplayPayload::Absolute {
                limit: 128_000,
                used: 32_000,
                percent: 25.0,
            },
        );

        assert_eq!(text, "gpt-4o · 32k/128k tokens (25%)");
    }

    #[test]
    fn context_usage_summary_keeps_fractional_percent_when_needed() {
        let text = context_usage_summary(
            "deepseek-v4-flash",
            ContextBudgetDisplayPayload::Absolute {
                limit: 256_000,
                used: 1_200,
                percent: 0.5,
            },
        );

        assert_eq!(text, "deepseek-v4-flash · 1.2k/256k tokens (0.5%)");
    }

    #[test]
    fn context_usage_summary_relative_uses_used_tokens_only() {
        let text = context_usage_summary(
            "local/qwen3",
            ContextBudgetDisplayPayload::Relative { used: 1_200 },
        );

        assert_eq!(text, "local/qwen3 · 1.2k tokens");
    }

    #[test]
    fn format_compact_tokens_uses_compact_suffix_style() {
        assert_eq!(format_compact_tokens(999), "999");
        assert_eq!(format_compact_tokens(1_000), "1k");
        assert_eq!(format_compact_tokens(1_700), "1.7k");
    }

    #[test]
    fn projection_error_message_uses_stable_short_copy() {
        let text = context_budget_error_message(&ContextBudgetLoadErrorPayload::ProjectionFailed {
            kind: runtime_domain::session::ContextBudgetProjectionErrorKind::Protocol,
            status: None,
            detail: Some("protocol error: inconsistent message fragments".to_string()),
        });

        assert_eq!(
            text,
            "Failed to build context budget from provider payload."
        );
    }

    #[test]
    fn category_labels_match_context_budget_source_buckets() {
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::SystemPrompt),
            "System prompt"
        );
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::ToolDefinitions),
            "Tool definitions"
        );
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::Messages),
            "Messages"
        );
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::FreeSpace),
            "Free space"
        );
    }

    fn segment(
        kind: SegmentKind,
        stack_order: usize,
        estimated_tokens: usize,
    ) -> ContextBudgetSegmentPayload {
        ContextBudgetSegmentPayload {
            kind,
            stack_order,
            estimated_tokens,
        }
    }
}
