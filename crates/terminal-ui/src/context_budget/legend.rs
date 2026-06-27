use ratatui::{buffer::Buffer, style::Style};
use runtime_domain::session::ContextBudgetSnapshotPayload;

use super::{
    segment_colors::context_budget_color_for_kind,
    state::{build_legend_entries, segment_kind_from_tag, segment_share_percent},
};
use crate::{
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, primary_text_style, tertiary_text_style},
};

const LEGEND_SWATCH_SYMBOL: &str = "■";
const LEGEND_SWATCH_WIDTH: usize = 1;
const LEGEND_SWATCH_GAP: usize = 1;
const LEGEND_PERCENT_GAP: usize = 1;
const LEGEND_PERCENT_WIDTH: usize = 6;

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
    let total_tokens = snapshot.total_estimated_tokens;
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

    let percent = segment_share_percent(entry.estimated_tokens, total_tokens);
    let percent_text = format!("{percent:>5.1}%");
    let fixed_width =
        LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP + LEGEND_PERCENT_GAP + LEGEND_PERCENT_WIDTH;
    let label_width = row_width.saturating_sub(fixed_width);
    let label = truncate_display_width_with_ellipsis(&entry.label, label_width.max(1));
    let label_x = area.x + u16::try_from(LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP).unwrap_or(0);
    let percent_x =
        area.x + u16::try_from(row_width.saturating_sub(LEGEND_PERCENT_WIDTH)).unwrap_or(u16::MAX);

    let color = context_budget_color_for_kind(segment_kind_from_tag(&entry.kind_tag), &palette);

    if let Some(cell) = buffer.cell_mut((area.x, y)) {
        cell.set_symbol(LEGEND_SWATCH_SYMBOL);
        cell.set_style(Style::new().fg(color));
    }
    buffer.set_string(label_x, y, label, primary_text_style(palette));
    buffer.set_string(percent_x, y, percent_text, tertiary_text_style(palette));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;
    use runtime_domain::session::ContextBudgetSegmentPayload;

    #[test]
    fn legend_uses_single_column_layout() {
        let area = ratatui::layout::Rect::new(0, 0, 28, 4);
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "model".to_string(),
            segments: vec![
                segment("assistant", 200, "assistant history"),
                segment("system", 160, "system prompt"),
                segment("user", 120, "user history"),
                segment("reasoning", 60, "reasoning"),
            ],
            total_estimated_tokens: 540,
            context_limit: None,
            display: runtime_domain::session::ContextBudgetDisplayPayload::Relative { used: 540 },
        };
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 28, 4));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

        let first_row = row_text(&buffer, 0);
        let second_row = row_text(&buffer, 1);
        let third_row = row_text(&buffer, 2);
        assert!(
            first_row.contains("system"),
            "legend should use stable semantic order instead of size order: {first_row:?}"
        );
        assert!(
            second_row.contains("user"),
            "second row should continue vertically with canonical category labels: {second_row:?}"
        );
        assert!(
            third_row.contains("assistant"),
            "subsequent rows should keep the same single legend column: {third_row:?}"
        );
    }

    #[test]
    fn legend_merges_duplicate_categories_and_shows_share_of_used_tokens() {
        let area = ratatui::layout::Rect::new(0, 0, 28, 3);
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
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 28, 3));

        render_context_budget_legend(&mut buffer, area, &snapshot, default_palette());

        let rows = (0..3).map(|row| row_text(&buffer, row)).collect::<Vec<_>>();
        assert_eq!(
            rows.iter().filter(|row| row.contains("user")).count(),
            1,
            "duplicate user segments should be merged into one legend row: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("assistant") && row.contains(" 50.0%")),
            "assistant share should be based on used tokens: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("user") && row.contains(" 50.0%")),
            "merged user share should be based on used tokens: {rows:?}"
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
