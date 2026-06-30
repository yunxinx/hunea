use runtime_domain::context_budget::{ContextBudgetSnapshot, ContextWindowUsage, SegmentKind};

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
pub(crate) fn context_usage_summary(model_id: &str, usage: ContextWindowUsage) -> String {
    let used = format_compact_tokens(usage.used as usize);
    let percent = format_compact_percent(usage.percent);
    if usage.is_saturated {
        return format!(
            "{model_id} · {used}+/{} tokens ({percent}+)",
            format_compact_tokens(usage.limit.get() as usize),
        );
    }

    format!(
        "{model_id} · {}/{} tokens ({})",
        used,
        format_compact_tokens(usage.limit.get() as usize),
        percent,
    )
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

pub(crate) fn free_space_tokens(snapshot: &ContextBudgetSnapshot) -> Option<usize> {
    Some(
        usize::try_from(
            snapshot
                .usage
                .limit
                .get()
                .saturating_sub(snapshot.usage.used),
        )
        .unwrap_or(usize::MAX),
    )
}

pub(crate) fn legend_share_total(snapshot: &ContextBudgetSnapshot) -> usize {
    usize::try_from(snapshot.usage.limit.get()).unwrap_or(usize::MAX)
}

pub(crate) fn aggregated_category_totals(
    snapshot: &ContextBudgetSnapshot,
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
    snapshot: &ContextBudgetSnapshot,
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
    use runtime_domain::context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
    };

    fn limit(value: u32) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn build_legend_entries_merges_duplicate_segment_kinds() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::UserMessage, 0, 120),
                segment(SegmentKind::AssistantMessage, 1, 200),
                segment(SegmentKind::UserMessage, 2, 80),
                segment(SegmentKind::Reasoning, 3, 40),
            ],
            total_estimated_tokens: 440,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 440,
                percent: 44.0,
                is_saturated: false,
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
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::System, 0, 100),
                segment(SegmentKind::AssistantMessage, 1, 120),
                segment(SegmentKind::UserMessage, 2, 80),
                segment(SegmentKind::ToolResult, 3, 40),
                segment(SegmentKind::ToolDefinitions, 4, 20),
            ],
            total_estimated_tokens: 360,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 360,
                percent: 36.0,
                is_saturated: false,
            },
        };

        let totals = aggregated_category_totals(&snapshot);

        assert_eq!(totals[0], (ContextBudgetCategoryKind::SystemPrompt, 100));
        assert_eq!(totals[1], (ContextBudgetCategoryKind::ToolDefinitions, 20));
        assert_eq!(totals[2], (ContextBudgetCategoryKind::Messages, 240));
    }

    #[test]
    fn legend_share_total_uses_context_limit_when_available() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 400,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 400,
                percent: 40.0,
                is_saturated: false,
            },
        };

        assert_eq!(legend_share_total(&snapshot), 1_000);
    }

    #[test]
    fn free_space_tokens_uses_remaining_context_capacity() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 400,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 400,
                percent: 40.0,
                is_saturated: false,
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
            ContextWindowUsage {
                limit: limit(128_000),
                used: 32_000,
                percent: 25.0,
                is_saturated: false,
            },
        );

        assert_eq!(text, "gpt-4o · 32k/128k tokens (25%)");
    }

    #[test]
    fn context_usage_summary_keeps_fractional_percent_when_needed() {
        let text = context_usage_summary(
            "deepseek-v4-flash",
            ContextWindowUsage {
                limit: limit(256_000),
                used: 1_200,
                percent: 0.5,
                is_saturated: false,
            },
        );

        assert_eq!(text, "deepseek-v4-flash · 1.2k/256k tokens (0.5%)");
    }

    #[test]
    fn context_usage_summary_uses_documented_absolute_limit() {
        let text = context_usage_summary(
            "local/qwen3",
            ContextWindowUsage {
                limit: limit(256_000),
                used: 1_200,
                percent: 0.5,
                is_saturated: false,
            },
        );

        assert_eq!(text, "local/qwen3 · 1.2k/256k tokens (0.5%)");
    }

    #[test]
    fn context_usage_summary_marks_saturated_usage_as_lower_bound() {
        let text = context_usage_summary(
            "local/qwen3",
            ContextWindowUsage {
                limit: limit(256_000),
                used: u32::MAX,
                percent: 1_677_721.5,
                is_saturated: true,
            },
        );

        assert_eq!(text, "local/qwen3 · 4294967.3k+/256k tokens (1677721.5%+)");
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

    fn segment(kind: SegmentKind, stack_order: usize, estimated_tokens: usize) -> ContextSegment {
        ContextSegment {
            kind,
            stack_order,
            estimated_tokens,
        }
    }
}
