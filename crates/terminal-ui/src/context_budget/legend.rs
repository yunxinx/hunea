use ratatui::{buffer::Buffer, style::Style};
use runtime_domain::session::ContextBudgetSnapshotPayload;

use super::{
    segment_colors::context_budget_color_for_category,
    state::{
        ContextBudgetCategoryKind, build_legend_entries, format_compact_tokens, legend_share_total,
        segment_share_percent,
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

pub(super) fn render_context_budget_legend(
    buffer: &mut Buffer,
    area: ratatui::layout::Rect,
    snapshot: &ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let entries = build_legend_entries(snapshot);
    let total_tokens = legend_share_total(snapshot);
    for (row, entry) in entries.iter().enumerate() {
        if row >= usize::from(area.height) {
            break;
        }

        render_legend_row(buffer, area, row, entry, total_tokens, palette);
    }
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
    use runtime_domain::session::ContextBudgetSegmentPayload;

    #[test]
    fn legend_keeps_stable_source_bucket_order_for_non_zero_categories() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 4);
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment("assistant", 200, "assistant history"),
                segment("system", 160, "system prompt"),
                segment("user", 120, "user history"),
                segment("reasoning", 60, "reasoning"),
            ],
            total_estimated_tokens: 540,
            context_limit: Some(1_000),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 540,
                percent: 54.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 4));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

        let first_row = row_text(&buffer, 0);
        assert!(
            first_row.contains("System prompt"),
            "legend should start with the system bucket: {first_row:?}"
        );
        assert!(
            !first_row.contains("Tool definitions"),
            "zero-token tool definitions should not render at all: {first_row:?}"
        );
        let second_row = row_text(&buffer, 1);
        assert!(
            second_row.contains("Messages"),
            "legend should aggregate message roles into one Messages row after the system row: {second_row:?}"
        );
        let third_row = row_text(&buffer, 2);
        assert!(
            third_row.contains("Free space"),
            "legend should include the free-space row when capacity remains: {third_row:?}"
        );
    }

    #[test]
    fn legend_uses_natural_token_and_percent_copy() {
        let area = ratatui::layout::Rect::new(0, 0, 48, 4);
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment("user", 120, "user"),
                segment("assistant", 200, "assistant"),
                segment("user", 80, "user"),
            ],
            total_estimated_tokens: 400,
            context_limit: Some(1_000),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 400,
                percent: 40.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 4));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

        let rows = (0..4).map(|row| row_text(&buffer, row)).collect::<Vec<_>>();
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
        let area = ratatui::layout::Rect::new(0, 0, 48, 4);
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![segment("assistant", 400, "assistant")],
            total_estimated_tokens: 400,
            context_limit: Some(400),
            display: runtime_domain::session::ContextBudgetDisplayPayload::Absolute {
                limit: 400,
                used: 400,
                percent: 100.0,
            },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 48, 4));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

        let rows = (0..4).map(|row| row_text(&buffer, row)).collect::<Vec<_>>();
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
        kind_tag: &str,
        estimated_tokens: usize,
        label: &str,
    ) -> ContextBudgetSegmentPayload {
        ContextBudgetSegmentPayload {
            kind_tag: kind_tag.to_string(),
            stack_order: 0,
            estimated_tokens,
            label: label.to_string(),
        }
    }
}
