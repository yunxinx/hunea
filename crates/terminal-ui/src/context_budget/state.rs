use runtime_domain::context_budget::SegmentKind;
use runtime_domain::session::{ContextBudgetDisplayPayload, ContextBudgetSnapshotPayload};

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
    pub(crate) error: Option<String>,
    pub(crate) snapshot: Option<ContextBudgetSnapshotPayload>,
}

impl Default for ContextBudgetState {
    fn default() -> Self {
        Self {
            revision: 1,
            loading: true,
            error: None,
            snapshot: None,
        }
    }
}

impl ContextBudgetState {
    pub(crate) fn apply_snapshot(&mut self, payload: ContextBudgetSnapshotPayload) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.error = None;
        self.snapshot = Some(payload);
    }

    pub(crate) fn set_error(&mut self, message: String) {
        self.revision = self.revision.saturating_add(1);
        self.loading = false;
        self.error = Some(message);
        self.snapshot = None;
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

pub(crate) fn header_summary(model_id: &str, display: ContextBudgetDisplayPayload) -> String {
    match display {
        ContextBudgetDisplayPayload::Relative { used } => {
            format!(
                "Context Usage · {model_id} · {} / ?",
                format_compact_tokens(used as usize)
            )
        }
        ContextBudgetDisplayPayload::Absolute {
            limit,
            used,
            percent,
        } => {
            format!(
                "Context Usage · {model_id} · {} / {} · {:.1}%",
                format_compact_tokens(used as usize),
                format_compact_tokens(limit as usize),
                percent
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
        let kind = segment_kind_from_tag(&segment.kind_tag);
        let category = context_budget_category_from_segment_kind(kind);
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

pub(crate) fn segment_kind_from_tag(tag: &str) -> SegmentKind {
    match tag {
        "system" => SegmentKind::System,
        "user" => SegmentKind::UserMessage,
        "assistant" => SegmentKind::AssistantMessage,
        "tool_result" => SegmentKind::ToolResult,
        "reasoning" => SegmentKind::Reasoning,
        "tools" => SegmentKind::ToolDefinitions,
        _ => SegmentKind::System,
    }
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
                segment("user", 0, 120),
                segment("assistant", 1, 200),
                segment("user", 2, 80),
                segment("reasoning", 3, 40),
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
                segment("system", 0, 100),
                segment("assistant", 1, 120),
                segment("user", 2, 80),
                segment("tool_result", 3, 40),
                segment("tools", 4, 20),
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
    fn format_compact_tokens_uses_compact_suffix_style() {
        assert_eq!(format_compact_tokens(999), "999");
        assert_eq!(format_compact_tokens(1_000), "1k");
        assert_eq!(format_compact_tokens(1_700), "1.7k");
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
        kind_tag: &str,
        stack_order: u16,
        estimated_tokens: usize,
    ) -> ContextBudgetSegmentPayload {
        ContextBudgetSegmentPayload {
            kind_tag: kind_tag.to_string(),
            stack_order,
            estimated_tokens,
            label: kind_tag.to_string(),
        }
    }
}
