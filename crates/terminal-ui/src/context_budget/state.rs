use runtime_domain::context_budget::SegmentKind;
use runtime_domain::session::{ContextBudgetDisplayPayload, ContextBudgetSnapshotPayload};

/// Legend row aggregated by stable context category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContextBudgetLegendEntry {
    pub(crate) kind_tag: String,
    pub(crate) label: String,
    pub(crate) estimated_tokens: usize,
    pub(crate) first_stack_order: u16,
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

pub(crate) fn format_compact_tokens(tokens: u32) -> String {
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
                "Context budget · {model_id} · {} / ?",
                format_compact_tokens(used)
            )
        }
        ContextBudgetDisplayPayload::Absolute {
            limit,
            used,
            percent,
        } => {
            format!(
                "Context budget · {model_id} · {} / {} · {:.1}%",
                format_compact_tokens(used),
                format_compact_tokens(limit),
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

pub(crate) fn build_legend_entries(
    snapshot: &ContextBudgetSnapshotPayload,
) -> Vec<ContextBudgetLegendEntry> {
    let mut entries: Vec<ContextBudgetLegendEntry> = Vec::new();

    for segment in &snapshot.segments {
        if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.kind_tag == segment.kind_tag)
        {
            existing.estimated_tokens = existing
                .estimated_tokens
                .saturating_add(segment.estimated_tokens);
            existing.first_stack_order = existing.first_stack_order.min(segment.stack_order);
            continue;
        }

        entries.push(ContextBudgetLegendEntry {
            kind_tag: segment.kind_tag.clone(),
            label: segment_kind_from_tag(&segment.kind_tag)
                .default_label()
                .to_string(),
            estimated_tokens: segment.estimated_tokens,
            first_stack_order: segment.stack_order,
        });
    }

    entries.sort_by(|a, b| {
        b.estimated_tokens
            .cmp(&a.estimated_tokens)
            .then_with(|| a.first_stack_order.cmp(&b.first_stack_order))
    });
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

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].kind_tag, "user");
        assert_eq!(entries[0].estimated_tokens, 200);
        assert_eq!(entries[1].kind_tag, "assistant");
        assert_eq!(entries[1].estimated_tokens, 200);
        assert_eq!(entries[0].first_stack_order, 0);
    }

    #[test]
    fn segment_share_percent_uses_used_token_total() {
        let percent = segment_share_percent(200, 500);
        assert!((percent - 40.0).abs() < f32::EPSILON);
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
