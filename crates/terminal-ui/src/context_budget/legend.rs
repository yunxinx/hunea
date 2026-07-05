use ratatui::{
    style::Style,
    text::{Line, Span},
};
use runtime_domain::context_budget::ContextBudgetSnapshot;

use super::{
    CONTEXT_BUDGET_LEGEND_SWATCH_GAP, CONTEXT_BUDGET_LEGEND_SWATCH_WIDTH,
    CONTEXT_BUDGET_SECTION_GAP_ROWS, blank_line,
    segment_colors::context_budget_color_for_category,
    summary::{
        ContextBudgetCategoryKind, build_legend_entries, context_usage_summary,
        format_compact_tokens, format_percent, legend_share_total,
    },
};
use crate::{
    display_width::display_width,
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, primary_text_style, tertiary_text_style},
};
use runtime_domain::context_budget::share_of_total_percent;

const LEGEND_SWATCH_SYMBOL: &str = "◼";
const LEGEND_FREE_SYMBOL: &str = "⛶";
const LEGEND_BODY_START_ROW: usize = 1 + CONTEXT_BUDGET_SECTION_GAP_ROWS;

pub(super) fn build_context_budget_legend_lines(
    area: ratatui::layout::Rect,
    snapshot: &ContextBudgetSnapshot,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let row_width = usize::from(area.width);
    let row_count = usize::from(area.height);
    let mut lines = Vec::with_capacity(row_count);
    lines.push(build_legend_summary_row_line(row_width, snapshot, palette));
    if row_count <= LEGEND_BODY_START_ROW {
        while lines.len() < row_count {
            lines.push(blank_line(row_width));
        }
        return lines;
    }
    for _ in 0..CONTEXT_BUDGET_SECTION_GAP_ROWS {
        lines.push(blank_line(row_width));
    }
    let entries = build_legend_entries(snapshot);
    let total_tokens = legend_share_total(snapshot);
    for entry in &entries {
        if lines.len() >= row_count {
            break;
        }
        lines.push(build_legend_row_line(
            row_width,
            entry,
            total_tokens,
            palette,
        ));
    }
    while lines.len() < row_count {
        lines.push(blank_line(row_width));
    }
    lines
}

fn build_legend_summary_row_line(
    row_width: usize,
    snapshot: &ContextBudgetSnapshot,
    palette: TerminalPalette,
) -> Line<'static> {
    let text = context_usage_summary(&snapshot.model_id, snapshot.usage);
    let truncated = truncate_display_width_with_ellipsis(&text, row_width.max(1));
    padded_styled_line(truncated, row_width, tertiary_text_style(palette))
}

fn build_legend_row_line(
    row_width: usize,
    entry: &super::summary::ContextBudgetLegendEntry,
    total_tokens: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    if row_width == 0 {
        return Line::raw("");
    }

    let text_width = row_width
        .saturating_sub(CONTEXT_BUDGET_LEGEND_SWATCH_WIDTH + CONTEXT_BUDGET_LEGEND_SWATCH_GAP);
    let prefix = format!("{}: ", entry.label);
    let prefix_width = display_width(&prefix);
    let detail = legend_detail_text(entry, total_tokens);

    let color = context_budget_color_for_category(entry.kind, &palette);
    let mut spans = vec![Span::styled(
        legend_symbol(entry.kind).to_string(),
        Style::new().fg(color),
    )];
    let mut rendered_width = CONTEXT_BUDGET_LEGEND_SWATCH_WIDTH;

    if row_width > CONTEXT_BUDGET_LEGEND_SWATCH_WIDTH {
        spans.push(Span::raw(" "));
        rendered_width += CONTEXT_BUDGET_LEGEND_SWATCH_GAP;
    }

    if prefix_width >= text_width {
        let truncated = truncate_display_width_with_ellipsis(&prefix, text_width.max(1));
        rendered_width += display_width(&truncated);
        spans.push(Span::styled(truncated, primary_text_style(palette)));
        if rendered_width < row_width {
            spans.push(Span::raw(" ".repeat(row_width - rendered_width)));
        }
        return Line::from(spans);
    }

    rendered_width += prefix_width;
    spans.push(Span::styled(prefix, primary_text_style(palette)));
    let detail_width = text_width.saturating_sub(prefix_width);
    let truncated_detail = truncate_display_width_with_ellipsis(&detail, detail_width.max(1));
    rendered_width += display_width(&truncated_detail);
    spans.push(Span::styled(truncated_detail, tertiary_text_style(palette)));
    if rendered_width < row_width {
        spans.push(Span::raw(" ".repeat(row_width - rendered_width)));
    }
    Line::from(spans)
}

fn legend_symbol(kind: ContextBudgetCategoryKind) -> &'static str {
    match kind {
        ContextBudgetCategoryKind::FreeSpace => LEGEND_FREE_SYMBOL,
        ContextBudgetCategoryKind::SystemPrompt
        | ContextBudgetCategoryKind::SkillDiscovery
        | ContextBudgetCategoryKind::ToolDefinitions
        | ContextBudgetCategoryKind::Messages => LEGEND_SWATCH_SYMBOL,
    }
}

fn legend_detail_text(
    entry: &super::summary::ContextBudgetLegendEntry,
    total_tokens: usize,
) -> String {
    let percent = share_of_total_percent(entry.estimated_tokens, total_tokens);
    format!(
        "~{} tokens ({})",
        format_compact_tokens(entry.estimated_tokens),
        format_percent(percent),
    )
}

fn padded_styled_line(text: String, width: usize, style: Style) -> Line<'static> {
    let text_width = display_width(&text);
    let mut spans = vec![Span::styled(text, style)];
    if text_width < width {
        spans.push(Span::raw(" ".repeat(width - text_width)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;
    use runtime_domain::context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
    };

    fn limit(value: usize) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn build_legend_lines_fill_requested_area_width() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![segment(SegmentKind::AssistantMessage, 400)],
            total_estimated_tokens: 400,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 400,
            },
        };
        let lines = build_context_budget_legend_lines(
            ratatui::layout::Rect::new(0, 0, 24, 4),
            &snapshot,
            default_palette(),
        );

        assert_eq!(lines.len(), 4);
        assert!(lines.iter().all(|line| line.width() == 24));
    }

    #[test]
    fn legend_detail_marks_category_tokens_as_approximate() {
        let entry = super::super::summary::ContextBudgetLegendEntry {
            kind: ContextBudgetCategoryKind::SystemPrompt,
            label: "System prompt".to_string(),
            estimated_tokens: 2_700,
        };

        assert_eq!(legend_detail_text(&entry, 256_000), "~2.7k tokens (1.1%)");
    }

    fn segment(kind: SegmentKind, estimated_tokens: usize) -> ContextSegment {
        ContextSegment {
            kind,
            estimated_tokens,
        }
    }
}
