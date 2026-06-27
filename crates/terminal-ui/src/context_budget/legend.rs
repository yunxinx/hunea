use ratatui::{buffer::Buffer, style::Style};
use runtime_domain::session::{ContextBudgetSegmentPayload, ContextBudgetSnapshotPayload};

use super::{
    layout::{LegendColumns, legend_slot_for_rank},
    segment_colors::context_budget_color_for_kind,
    state::{segment_kind_from_tag, segment_share_percent, sorted_legend_indices},
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
    columns: LegendColumns,
    snapshot: &ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) {
    if columns.left.width == 0 || columns.left.height == 0 {
        return;
    }

    let indices = sorted_legend_indices(&snapshot.segments);
    let total_tokens = snapshot.total_estimated_tokens;
    for (rank, segment_index) in indices.into_iter().enumerate() {
        let (column_index, row) = legend_slot_for_rank(rank, columns.rows_per_column);
        let area = match legend_column_rect(columns, column_index) {
            Some(area) => area,
            None => break,
        };
        if row >= usize::from(area.height) {
            continue;
        }

        render_legend_row(
            buffer,
            area,
            row,
            &snapshot.segments[segment_index],
            total_tokens,
            snapshot.display,
            palette,
        );
    }
}

fn render_legend_row(
    buffer: &mut Buffer,
    area: ratatui::layout::Rect,
    row: usize,
    segment: &ContextBudgetSegmentPayload,
    total_tokens: usize,
    display: runtime_domain::session::ContextBudgetDisplayPayload,
    palette: TerminalPalette,
) {
    let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
    let row_width = usize::from(area.width);
    if row_width == 0 {
        return;
    }

    let percent = segment_share_percent(segment.estimated_tokens, total_tokens, display);
    let percent_text = format!("{percent:>5.1}%");
    let fixed_width =
        LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP + LEGEND_PERCENT_GAP + LEGEND_PERCENT_WIDTH;
    let label_width = row_width.saturating_sub(fixed_width);
    let label = truncate_display_width_with_ellipsis(&segment.label, label_width.max(1));
    let label_x = area.x + u16::try_from(LEGEND_SWATCH_WIDTH + LEGEND_SWATCH_GAP).unwrap_or(0);
    let percent_x =
        area.x + u16::try_from(row_width.saturating_sub(LEGEND_PERCENT_WIDTH)).unwrap_or(u16::MAX);

    let color = context_budget_color_for_kind(segment_kind_from_tag(&segment.kind_tag), &palette);

    if let Some(cell) = buffer.cell_mut((area.x, y)) {
        cell.set_symbol(LEGEND_SWATCH_SYMBOL);
        cell.set_style(Style::new().fg(color));
    }
    buffer.set_string(label_x, y, label, primary_text_style(palette));
    buffer.set_string(percent_x, y, percent_text, tertiary_text_style(palette));
}

fn legend_column_rect(
    columns: LegendColumns,
    column_index: usize,
) -> Option<ratatui::layout::Rect> {
    match column_index {
        0 => Some(columns.left),
        1 => columns.right,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;

    #[test]
    fn legend_uses_column_major_layout_left_then_right() {
        let columns = LegendColumns {
            left: ratatui::layout::Rect::new(0, 0, 28, 2),
            right: Some(ratatui::layout::Rect::new(32, 0, 28, 2)),
            rows_per_column: 2,
        };
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
        let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 2));

        render_context_budget_legend(&mut buffer, columns, &snapshot, default_palette());

        let first_row = row_text(&buffer, 0);
        let second_row = row_text(&buffer, 1);
        assert!(
            first_row.contains("assistant history") && first_row.contains("user history"),
            "left column should fill before the right column: {first_row:?}"
        );
        assert!(
            second_row.contains("system prompt") && second_row.contains("reasoning"),
            "second row should contain the next left item and then the next right item: {second_row:?}"
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
