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
        "{} tokens ({})",
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
    use ratatui::buffer::Buffer;
    use runtime_domain::context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
    };

    fn limit(value: u32) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn legend_keeps_stable_source_bucket_order_for_non_zero_categories() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 5);
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::AssistantMessage, 200),
                segment(SegmentKind::System, 160),
                segment(SegmentKind::UserMessage, 120),
                segment(SegmentKind::Reasoning, 60),
            ],
            total_estimated_tokens: 540,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 540,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));
        let lines = build_context_budget_legend_lines(area, &snapshot, default_palette());
        for (row, line) in lines.iter().enumerate() {
            buffer.set_line(0, u16::try_from(row).unwrap_or(0), line, area.width);
        }

        let first_row = row_text(&buffer, 0);
        assert!(
            first_row.contains("model · 540/1k tokens"),
            "legend first row should keep the model name and use legend-style usage copy: {first_row:?}"
        );
        assert_eq!(
            buffer[(0, 0)].fg,
            default_palette().tertiary,
            "summary row should use the weaker tertiary text color"
        );
        let second_row = row_text(&buffer, 1);
        assert!(
            second_row.trim().is_empty(),
            "legend should leave one blank row below the summary: {second_row:?}"
        );
        let third_row = row_text(&buffer, LEGEND_BODY_START_ROW as u16);
        assert!(
            third_row.contains("System prompt"),
            "legend should start category rows with the system bucket after the blank spacer row: {third_row:?}"
        );
        assert!(
            !third_row.contains("Tool definitions"),
            "zero-token tool definitions should not render at all: {third_row:?}"
        );
        let fourth_row = row_text(&buffer, 3);
        assert!(
            fourth_row.contains("Messages"),
            "legend should aggregate message roles into one Messages row after the system row: {fourth_row:?}"
        );
        let fifth_row = row_text(&buffer, 4);
        assert!(
            fifth_row.contains("Free space"),
            "legend should include the free-space row when capacity remains: {fifth_row:?}"
        );
    }

    #[test]
    fn legend_uses_natural_token_and_percent_copy() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 5);
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::UserMessage, 120),
                segment(SegmentKind::AssistantMessage, 200),
                segment(SegmentKind::UserMessage, 80),
            ],
            total_estimated_tokens: 400,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 400,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));
        let lines = build_context_budget_legend_lines(area, &snapshot, default_palette());
        for (row, line) in lines.iter().enumerate() {
            buffer.set_line(0, u16::try_from(row).unwrap_or(0), line, area.width);
        }

        let rows = (0..5).map(|row| row_text(&buffer, row)).collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row.contains("model · 400/1k")),
            "summary row should keep the model name and use the same usage copy style as the legend: {rows:?}"
        );
        assert!(
            rows.get(1).is_some_and(|row| row.trim().is_empty()),
            "summary row should be visually separated from legend entries by one blank row: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Messages: 400 tokens (40.0%)")),
            "messages row should use compact natural-language token copy: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Free space: 600 tokens (60.0%)")),
            "free-space row should use the same full-limit percent baseline: {rows:?}"
        );
    }

    #[test]
    fn legend_hides_zero_token_categories() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 5);
        let snapshot = ContextBudgetSnapshot {
            model_id: "model".to_string(),
            segments: vec![segment(SegmentKind::AssistantMessage, 400)],
            total_estimated_tokens: 400,
            usage: ContextWindowUsage {
                limit: limit(400),
                used: 400,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));
        let lines = build_context_budget_legend_lines(area, &snapshot, default_palette());
        for (row, line) in lines.iter().enumerate() {
            buffer.set_line(0, u16::try_from(row).unwrap_or(0), line, area.width);
        }

        let rows = (0..5).map(|row| row_text(&buffer, row)).collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row.contains("model · 400/400")),
            "summary row should keep the model name with legend-style absolute usage copy when full: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Messages: 400 tokens (100.0%)")),
            "non-zero message categories should still render: {rows:?}"
        );
        assert!(
            rows.iter().all(|row| !row.contains("System prompt")),
            "zero-token system categories should be omitted entirely: {rows:?}"
        );
        assert!(
            rows.iter().all(|row| !row.contains("Free space")),
            "zero-token free space should also be omitted: {rows:?}"
        );
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

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    fn segment(kind: SegmentKind, estimated_tokens: usize) -> ContextSegment {
        ContextSegment {
            kind,
            estimated_tokens,
        }
    }
}
