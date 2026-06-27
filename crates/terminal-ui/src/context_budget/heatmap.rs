//! Heatmap cell allocation for context budget segments.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use runtime_domain::context_budget::ContextSegment;
use runtime_domain::session::ContextBudgetSnapshotPayload;

use super::{
    segment_colors::{context_budget_color_for_kind, context_budget_empty_color},
    state::segment_kind_from_tag,
};
use crate::theme::TerminalPalette;

const HEATMAP_CELL_SYMBOL: &str = "■";
const HEATMAP_CELL_WIDTH: usize = 2;

/// Assigns grid cells to segments proportional to token share (largest remainder).
/// Returns per-segment cell counts in segment slice order (`stack_order` order).
pub(crate) fn allocate_heatmap_cells(
    segments: &[ContextSegment],
    total_cells: usize,
) -> Vec<usize> {
    if total_cells == 0 || segments.is_empty() {
        return vec![0; segments.len()];
    }
    let total_tokens: usize = segments.iter().map(|s| s.estimated_tokens).sum();
    if total_tokens == 0 {
        let base = total_cells / segments.len();
        let mut counts = vec![base; segments.len()];
        for i in 0..(total_cells - base * segments.len()) {
            counts[i % segments.len()] += 1;
        }
        return counts;
    }

    let non_zero_indices: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| (segment.estimated_tokens > 0).then_some(index))
        .collect();
    let guarantee_visible = total_cells >= non_zero_indices.len();
    let mut counts = vec![0usize; segments.len()];
    let mut remainders = Vec::with_capacity(segments.len());
    let reserved_cells = if guarantee_visible {
        non_zero_indices.len()
    } else {
        0
    };
    let distributable_cells = total_cells.saturating_sub(reserved_cells);
    let mut assigned = reserved_cells;
    for (index, segment) in segments.iter().enumerate() {
        if guarantee_visible && segment.estimated_tokens > 0 {
            counts[index] = 1;
        }
    }

    for (index, segment) in segments.iter().enumerate() {
        let exact =
            (segment.estimated_tokens as f64 / total_tokens as f64) * distributable_cells as f64;
        let floor = exact.floor() as usize;
        if floor > 0 {
            counts[index] = counts[index].saturating_add(floor);
            assigned = assigned.saturating_add(floor);
        }
        remainders.push((exact - floor as f64, index, segment.estimated_tokens));
    }
    let mut leftover = total_cells.saturating_sub(assigned);
    remainders.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.2.cmp(&a.2))
    });
    for (_, index, _) in remainders {
        if leftover == 0 {
            break;
        }
        counts[index] += 1;
        leftover -= 1;
    }
    counts
}

pub(super) fn render_context_budget_heatmap(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) {
    if area.is_empty() {
        return;
    }

    let grid_columns = heatmap_grid_columns(area.width);
    let grid_rows = usize::from(area.height);
    let total_cells = grid_columns.saturating_mul(grid_rows);
    if total_cells == 0 {
        return;
    }

    clear_heatmap_area(buffer, area);

    let segments = ordered_segments_for_heatmap(snapshot);
    let occupied_cells = occupied_heatmap_cells(snapshot, total_cells, segments.len());
    let counts = allocate_heatmap_cells(&segments, occupied_cells);
    let mut fill_kinds = Vec::with_capacity(total_cells);
    for (segment, count) in segments.iter().zip(counts.iter()) {
        for _ in 0..*count {
            fill_kinds.push(Some(segment.kind));
        }
    }
    fill_kinds.resize(total_cells, None);

    let empty_color = context_budget_empty_color(&palette);
    for (cell_index, kind) in fill_kinds.into_iter().enumerate() {
        let row = cell_index / grid_columns;
        let column = cell_index % grid_columns;
        let x =
            area.x + u16::try_from(column.saturating_mul(HEATMAP_CELL_WIDTH)).unwrap_or(u16::MAX);
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if x >= area.x + area.width {
            continue;
        }

        let color = kind
            .map(|kind| context_budget_color_for_kind(kind, &palette))
            .unwrap_or(empty_color);
        if let Some(cell) = buffer.cell_mut((x, y)) {
            cell.set_symbol(HEATMAP_CELL_SYMBOL);
            cell.set_style(Style::new().fg(color));
        }
    }
}

pub(super) fn heatmap_grid_columns(width: u16) -> usize {
    usize::from(width.max(1)).div_ceil(HEATMAP_CELL_WIDTH)
}

fn occupied_heatmap_cells(
    snapshot: &ContextBudgetSnapshotPayload,
    total_cells: usize,
    segment_count: usize,
) -> usize {
    match snapshot.display {
        runtime_domain::session::ContextBudgetDisplayPayload::Relative { .. } => total_cells,
        runtime_domain::session::ContextBudgetDisplayPayload::Absolute { limit, used, .. } => {
            if limit == 0 || total_cells == 0 || used == 0 {
                return 0;
            }
            let exact = (used as f64 / limit as f64) * total_cells as f64;
            let occupied = exact.round() as usize;
            occupied.clamp(segment_count.min(total_cells), total_cells)
        }
    }
}

pub(super) fn ordered_segments_for_heatmap(
    snapshot: &ContextBudgetSnapshotPayload,
) -> Vec<ContextSegment> {
    let mut segments: Vec<ContextSegment> = snapshot
        .segments
        .iter()
        .map(|segment| ContextSegment {
            kind: segment_kind_from_tag(&segment.kind_tag),
            stack_order: segment.stack_order,
            estimated_tokens: segment.estimated_tokens,
            label: segment.label.clone(),
        })
        .collect();
    segments.sort_by_key(|segment| segment.stack_order);
    segments
}

fn clear_heatmap_area(buffer: &mut Buffer, area: Rect) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(" ");
                cell.set_style(Style::default());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::context_budget::{ContextSegment, SegmentKind};
    use runtime_domain::session::{ContextBudgetDisplayPayload, ContextBudgetSegmentPayload};

    use crate::theme::default_palette;

    fn seg(kind: SegmentKind, tokens: usize, order: u16) -> ContextSegment {
        ContextSegment {
            kind,
            stack_order: order,
            estimated_tokens: tokens,
            label: kind.default_label().to_string(),
        }
    }

    #[test]
    fn allocate_cells_sum_equals_grid_size() {
        let segments = vec![
            seg(SegmentKind::System, 100, 0),
            seg(SegmentKind::UserMessage, 200, 1),
            seg(SegmentKind::AssistantMessage, 100, 2),
        ];
        let counts = allocate_heatmap_cells(&segments, 24);
        assert_eq!(counts.iter().sum::<usize>(), 24);
    }

    #[test]
    fn non_zero_segments_get_one_cell_when_grid_can_fit_all_segments() {
        let segments = vec![
            seg(SegmentKind::System, 1_000, 0),
            seg(SegmentKind::UserMessage, 1, 1),
            seg(SegmentKind::AssistantMessage, 1, 2),
        ];

        let counts = allocate_heatmap_cells(&segments, 3);

        assert_eq!(
            counts,
            vec![1, 1, 1],
            "each non-zero segment should stay visible when the grid can fit every segment"
        );
    }

    #[test]
    fn render_keeps_empty_capacity_cells_visible() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "local/qwen3".to_string(),
            segments: vec![ContextBudgetSegmentPayload {
                kind_tag: "system".to_string(),
                stack_order: 0,
                estimated_tokens: 10,
                label: "system".to_string(),
            }],
            total_estimated_tokens: 10,
            context_limit: Some(80),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 80,
                used: 10,
                percent: 12.5,
            },
        };
        let mut buffer = Buffer::empty(Rect::new(0, 0, 8, 2));

        render_context_budget_heatmap(
            &mut buffer,
            Rect::new(0, 0, 8, 2),
            &snapshot,
            default_palette(),
        );

        let empty_color = context_budget_empty_color(&default_palette());
        let empty_cells = buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == HEATMAP_CELL_SYMBOL && cell.fg == empty_color)
            .count();
        assert!(
            empty_cells > 0,
            "heatmap should paint remaining capacity cells explicitly instead of leaving blanks"
        );
    }
}
