use ratatui::{buffer::Buffer, style::Style};
use runtime_domain::session::ContextBudgetSnapshotPayload;

use super::{
    segment_colors::context_budget_color_for_category,
    state::{
        ContextBudgetCategoryKind, build_legend_entries, context_usage_summary,
        format_compact_tokens, legend_share_total, segment_share_percent,
    },
};
use crate::{
    display_width::display_width,
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, primary_text_style, tertiary_text_style},
};

const LEGEND_SWATCH_SYMBOL: &str = "◼";
const LEGEND_FREE_SYMBOL: &str = "⛶";
const LEGEND_SWATCH_WIDTH: usize = 1;
const LEGEND_SWATCH_GAP: usize = 1;
const LEGEND_BODY_START_ROW: usize = 2;

pub(super) fn render_context_budget_legend(
    buffer: &mut Buffer,
    area: ratatui::layout::Rect,
    snapshot: &ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    render_legend_summary_row(buffer, area, snapshot, palette);
    if area.height <= u16::try_from(LEGEND_BODY_START_ROW).unwrap_or(u16::MAX) {
        return;
    }

    let entries = build_legend_entries(snapshot);
    let total_tokens = legend_share_total(snapshot);
    for (row, entry) in entries.iter().enumerate() {
        let legend_row = row.saturating_add(LEGEND_BODY_START_ROW);
        if legend_row >= usize::from(area.height) {
            break;
        }

        render_legend_row(buffer, area, legend_row, entry, total_tokens, palette);
    }
}

fn render_legend_summary_row(
    buffer: &mut Buffer,
    area: ratatui::layout::Rect,
    snapshot: &ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) {
    let text = context_usage_summary(&snapshot.model_id, snapshot.display);
    let truncated = truncate_display_width_with_ellipsis(&text, usize::from(area.width).max(1));
    buffer.set_string(area.x, area.y, truncated, tertiary_text_style(palette));
}

fn render_legend_row(
    buffer: &mut Buffer,
    area: ratatui::layout::Rect,
    row: usize,
    entry: &super::state::ContextBudgetLegendEntry,
    total_tokens: usize,
    palette: TerminalPalette,
) {
    let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
    let row_width = usize::from(area.width);
    if row_width == 0 {
        return;
    }

    let text_x = area.x + u16::try_from(LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP).unwrap_or(0);
    let text_width = row_width.saturating_sub(LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP);
    let prefix = format!("{}: ", entry.label);
    let prefix_width = display_width(&prefix);
    let detail = legend_detail_text(entry, total_tokens);

    let color = context_budget_color_for_category(entry.kind, &palette);

    if let Some(cell) = buffer.cell_mut((area.x, y)) {
        cell.set_symbol(legend_symbol(entry.kind));
        cell.set_style(Style::new().fg(color));
    }

    if prefix_width >= text_width {
        let truncated = truncate_display_width_with_ellipsis(&prefix, text_width.max(1));
        buffer.set_string(text_x, y, truncated, primary_text_style(palette));
        return;
    }

    buffer.set_string(text_x, y, prefix, primary_text_style(palette));
    let detail_x = text_x + u16::try_from(prefix_width).unwrap_or(u16::MAX);
    let detail_width = text_width.saturating_sub(prefix_width);
    let truncated_detail = truncate_display_width_with_ellipsis(&detail, detail_width.max(1));
    buffer.set_string(detail_x, y, truncated_detail, tertiary_text_style(palette));
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
    entry: &super::state::ContextBudgetLegendEntry,
    total_tokens: usize,
) -> String {
    let percent = segment_share_percent(entry.estimated_tokens, total_tokens);
    format!(
        "{} tokens ({percent:.1}%)",
        format_compact_tokens(entry.estimated_tokens)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;
    use runtime_domain::context_budget::SegmentKind;
    use runtime_domain::session::ContextBudgetSegmentPayload;

    #[test]
    fn legend_keeps_stable_source_bucket_order_for_non_zero_categories() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 5);
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::AssistantMessage, 200, "assistant history"),
                segment(SegmentKind::System, 160, "system prompt"),
                segment(SegmentKind::UserMessage, 120, "user history"),
                segment(SegmentKind::Reasoning, 60, "reasoning"),
            ],
            total_estimated_tokens: 540,
            context_limit: Some(1_000),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 540,
                percent: 54.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

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
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment(SegmentKind::UserMessage, 120, "user"),
                segment(SegmentKind::AssistantMessage, 200, "assistant"),
                segment(SegmentKind::UserMessage, 80, "user"),
            ],
            total_estimated_tokens: 400,
            context_limit: Some(1_000),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 400,
                percent: 40.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

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
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![segment(SegmentKind::AssistantMessage, 400, "assistant")],
            total_estimated_tokens: 400,
            context_limit: Some(400),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 400,
                used: 400,
                percent: 100.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 5));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

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

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    fn segment(
        kind: SegmentKind,
        estimated_tokens: usize,
        label: &str,
    ) -> ContextBudgetSegmentPayload {
        ContextBudgetSegmentPayload {
            kind,
            stack_order: 0,
            estimated_tokens,
            label: label.to_string(),
        }
    }
}
