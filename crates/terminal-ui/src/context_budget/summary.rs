use runtime_domain::context_budget::{ContextBudgetSnapshot, ContextWindowUsage, SegmentKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextBudgetCategoryKind {
    SystemPrompt,
    SkillDiscovery,
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

pub(crate) fn format_percent(percent: f32) -> String {
    let tenths = (percent * 10.0).round() as i32;
    let whole = tenths / 10;
    let fraction = tenths % 10;
    format!("{whole}.{fraction}%")
}

pub(crate) fn context_usage_percent(usage: ContextWindowUsage) -> f32 {
    (usage.used as f32 / usage.limit.get() as f32) * 100.0
}

/// `context_usage_summary` 返回右侧图示首行使用的模型与上下文摘要。
pub(crate) fn context_usage_summary(model_id: &str, usage: ContextWindowUsage) -> String {
    let used = format_compact_tokens(usage.used);
    let percent = format_percent(context_usage_percent(usage));

    format!(
        "{model_id} · ~{}/{} tokens ({})",
        used,
        format_compact_tokens(usage.limit.get()),
        percent,
    )
}

pub(crate) fn context_budget_category_display_rank(kind: ContextBudgetCategoryKind) -> usize {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => 0,
        ContextBudgetCategoryKind::SkillDiscovery => 1,
        ContextBudgetCategoryKind::ToolDefinitions => 2,
        ContextBudgetCategoryKind::Messages => 3,
        ContextBudgetCategoryKind::FreeSpace => 4,
    }
}

pub(crate) fn context_budget_category_label(kind: ContextBudgetCategoryKind) -> &'static str {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => "System prompt",
        ContextBudgetCategoryKind::SkillDiscovery => "Skill discovery",
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
        SegmentKind::SkillDiscovery => ContextBudgetCategoryKind::SkillDiscovery,
        SegmentKind::ToolDefinitions => ContextBudgetCategoryKind::ToolDefinitions,
        SegmentKind::UserMessage
        | SegmentKind::AssistantMessage
        | SegmentKind::ToolResult
        | SegmentKind::Reasoning => ContextBudgetCategoryKind::Messages,
    }
}

pub(crate) fn free_space_tokens(snapshot: &ContextBudgetSnapshot) -> usize {
    snapshot
        .usage
        .limit
        .get()
        .saturating_sub(snapshot.usage.used)
}

pub(crate) fn legend_share_total(snapshot: &ContextBudgetSnapshot) -> usize {
    snapshot.usage.limit.get()
}

pub(crate) fn aggregated_category_totals(
    snapshot: &ContextBudgetSnapshot,
) -> [(ContextBudgetCategoryKind, usize); 4] {
    let mut totals = [0usize; 4];

    for segment in &snapshot.segments {
        let category = context_budget_category_from_segment_kind(segment.kind);
        let rank = context_budget_category_display_rank(category);
        if rank < totals.len() {
            totals[rank] = totals[rank].saturating_add(segment.estimated_tokens);
        }
    }

    [
        (ContextBudgetCategoryKind::SystemPrompt, totals[0]),
        (ContextBudgetCategoryKind::SkillDiscovery, totals[1]),
        (ContextBudgetCategoryKind::ToolDefinitions, totals[2]),
        (ContextBudgetCategoryKind::Messages, totals[3]),
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

    let estimated_tokens = free_space_tokens(snapshot);
    if estimated_tokens > 0 {
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
        share_of_total_percent,
    };

    fn limit(value: usize) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn build_legend_entries_merges_duplicate_segment_kinds() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::UserMessage, 120),
                segment(SegmentKind::AssistantMessage, 200),
                segment(SegmentKind::UserMessage, 80),
                segment(SegmentKind::Reasoning, 40),
            ],
            total_estimated_tokens: 440,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 440,
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
                segment(SegmentKind::System, 100),
                segment(SegmentKind::SkillDiscovery, 30),
                segment(SegmentKind::AssistantMessage, 120),
                segment(SegmentKind::UserMessage, 80),
                segment(SegmentKind::ToolResult, 40),
                segment(SegmentKind::ToolDefinitions, 20),
            ],
            total_estimated_tokens: 390,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 390,
            },
        };

        let totals = aggregated_category_totals(&snapshot);

        assert_eq!(totals[0], (ContextBudgetCategoryKind::SystemPrompt, 100));
        assert_eq!(totals[1], (ContextBudgetCategoryKind::SkillDiscovery, 30));
        assert_eq!(totals[2], (ContextBudgetCategoryKind::ToolDefinitions, 20));
        assert_eq!(totals[3], (ContextBudgetCategoryKind::Messages, 240));
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
            },
        };

        assert_eq!(free_space_tokens(&snapshot), 600);
    }

    #[test]
    fn share_of_total_percent_uses_provided_total() {
        let percent = share_of_total_percent(200, 500);
        assert!((percent - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_usage_summary_uses_fixed_single_decimal_percent() {
        let text = context_usage_summary(
            "gpt-4o",
            ContextWindowUsage {
                limit: limit(128_000),
                used: 32_000,
            },
        );

        assert_eq!(text, "gpt-4o · ~32k/128k tokens (25.0%)");
    }

    #[test]
    fn context_usage_summary_keeps_fractional_percent_when_needed() {
        let text = context_usage_summary(
            "deepseek-v4-flash",
            ContextWindowUsage {
                limit: limit(256_000),
                used: 1_200,
            },
        );

        assert_eq!(text, "deepseek-v4-flash · ~1.2k/256k tokens (0.5%)");
    }

    #[test]
    fn context_usage_summary_uses_documented_absolute_limit() {
        let text = context_usage_summary(
            "local/qwen3",
            ContextWindowUsage {
                limit: limit(256_000),
                used: 1_200,
            },
        );

        assert_eq!(text, "local/qwen3 · ~1.2k/256k tokens (0.5%)");
    }

    #[test]
    fn context_usage_summary_displays_used_tokens_above_u32_max() {
        let text = context_usage_summary(
            "local/qwen3",
            ContextWindowUsage {
                limit: limit(256_000),
                used: usize::try_from(u32::MAX).expect("u32::MAX should fit in usize") + 1,
            },
        );

        assert_eq!(text, "local/qwen3 · ~4294967.3k/256k tokens (1677721.6%)");
    }

    #[test]
    fn format_compact_tokens_uses_compact_suffix_style() {
        assert_eq!(format_compact_tokens(999), "999");
        assert_eq!(format_compact_tokens(1_000), "1k");
        assert_eq!(format_compact_tokens(1_700), "1.7k");
    }

    #[test]
    fn format_percent_keeps_fixed_single_decimal_precision() {
        assert_eq!(format_percent(25.0), "25.0%");
        assert_eq!(format_percent(0.5), "0.5%");
    }

    #[test]
    fn category_labels_match_context_budget_source_buckets() {
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::SystemPrompt),
            "System prompt"
        );
        assert_eq!(
            context_budget_category_label(ContextBudgetCategoryKind::SkillDiscovery),
            "Skill discovery"
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

    fn segment(kind: SegmentKind, estimated_tokens: usize) -> ContextSegment {
        ContextSegment {
            kind,
            estimated_tokens,
        }
    }
}
