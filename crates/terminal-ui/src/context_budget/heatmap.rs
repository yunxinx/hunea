//! Heatmap cell allocation for context budget segments.

use ratatui::{buffer::Buffer, layout::Rect, style::Style};
use runtime_domain::session::ContextBudgetSnapshotPayload;

use super::{
    segment_colors::{context_budget_color_for_category, context_budget_empty_color},
    state::{ContextBudgetCategoryKind, aggregated_category_totals},
};
use crate::theme::TerminalPalette;

const HEATMAP_FULL_SYMBOL: &str = "◼";
const HEATMAP_EMPTY_SYMBOL: &str = "⛶";
const HEATMAP_CELL_WIDTH: usize = 2;
const HEATMAP_GRID_COLUMNS: usize = 10;
const HEATMAP_GRID_ROWS: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeatmapCellFill {
    kind: Option<ContextBudgetCategoryKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct HeatmapCategory {
    kind: ContextBudgetCategoryKind,
    estimated_tokens: usize,
}

/// Assigns grid cells to segments proportional to token share (largest remainder).
/// Returns per-segment cell counts in segment slice order (`stack_order` order).
pub(crate) fn allocate_heatmap_cells(
    segments: &[HeatmapCategory],
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
    let grid_rows = heatmap_grid_rows(area.height);
    let total_cells = grid_columns.saturating_mul(grid_rows);
    if total_cells == 0 {
        return;
    }

    clear_heatmap_area(buffer, area);

    let segments = aggregated_categories_for_heatmap(snapshot);
    let occupied_cells = occupied_heatmap_cells(snapshot, total_cells, segments.len());
    let counts = allocate_heatmap_cells(&segments, occupied_cells);
    let mut fill_kinds = Vec::with_capacity(total_cells);
    for (segment, count) in segments.iter().zip(counts.iter()) {
        for _ in 0..*count {
            fill_kinds.push(HeatmapCellFill {
                kind: Some(segment.kind),
            });
        }
    }
    fill_kinds.resize(total_cells, HeatmapCellFill { kind: None });

    let empty_color = context_budget_empty_color(&palette);
    for (cell_index, fill) in fill_kinds.into_iter().enumerate() {
        let row = cell_index / grid_columns;
        let column = cell_index % grid_columns;
        let x =
            area.x + u16::try_from(column.saturating_mul(HEATMAP_CELL_WIDTH)).unwrap_or(u16::MAX);
        let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
        if x >= area.x + area.width {
            continue;
        }

        let color = fill
            .kind
            .map(|kind| context_budget_color_for_category(kind, &palette))
            .unwrap_or(empty_color);
        render_heatmap_cell(buffer, x, y, area, fill, color);
    }
}

pub(super) fn heatmap_grid_columns(width: u16) -> usize {
    usize::from(width.max(1))
        .div_ceil(HEATMAP_CELL_WIDTH)
        .min(HEATMAP_GRID_COLUMNS)
}

fn heatmap_grid_rows(height: u16) -> usize {
    usize::from(height).min(HEATMAP_GRID_ROWS)
}

pub(super) fn occupied_heatmap_cells(
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

pub(super) fn aggregated_categories_for_heatmap(
    snapshot: &ContextBudgetSnapshotPayload,
) -> Vec<HeatmapCategory> {
    aggregated_category_totals(snapshot)
        .into_iter()
        .filter_map(|(kind, estimated_tokens)| {
            (estimated_tokens > 0).then_some(HeatmapCategory {
                kind,
                estimated_tokens,
            })
        })
        .collect()
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

fn render_heatmap_cell(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    area: Rect,
    fill: HeatmapCellFill,
    color: ratatui::style::Color,
) {
    let style = Style::new().fg(color);
    if let Some(cell) = buffer.cell_mut((x, y)) {
        cell.set_symbol(heatmap_symbol(fill));
        cell.set_style(style);
    }

    let spacer_x = x.saturating_add(1);
    if spacer_x < area.x + area.width
        && let Some(cell) = buffer.cell_mut((spacer_x, y))
    {
        cell.set_symbol(" ");
        cell.set_style(style);
    }
}

#[cfg(test)]
use ratatui::buffer::Cell;

#[cfg(test)]
pub(super) fn is_context_budget_heatmap_cell(cell: &Cell, palette: TerminalPalette) -> bool {
    let empty_color = context_budget_empty_color(&palette);
    let heatmap_symbols = [HEATMAP_FULL_SYMBOL, HEATMAP_EMPTY_SYMBOL];
    let heatmap_colors = [
        ContextBudgetCategoryKind::SystemPrompt,
        ContextBudgetCategoryKind::ToolDefinitions,
        ContextBudgetCategoryKind::Messages,
    ]
    .map(|kind| context_budget_color_for_category(kind, &palette));

    heatmap_symbols.contains(&cell.symbol())
        && (cell.fg == empty_color || heatmap_colors.contains(&cell.fg))
}

fn heatmap_symbol(fill: HeatmapCellFill) -> &'static str {
    match fill.kind {
        None => HEATMAP_EMPTY_SYMBOL,
        Some(_) => HEATMAP_FULL_SYMBOL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::context_budget::SegmentKind;
    use runtime_domain::session::{ContextBudgetDisplayPayload, ContextBudgetSegmentPayload};

    use crate::theme::default_palette;

    fn seg(kind: ContextBudgetCategoryKind, tokens: usize) -> HeatmapCategory {
        HeatmapCategory {
            kind,
            estimated_tokens: tokens,
        }
    }

    #[test]
    fn allocate_cells_sum_equals_grid_size() {
        let segments = vec![
            seg(ContextBudgetCategoryKind::SystemPrompt, 100),
            seg(ContextBudgetCategoryKind::ToolDefinitions, 80),
            seg(ContextBudgetCategoryKind::Messages, 220),
        ];
        let counts = allocate_heatmap_cells(&segments, 24);
        assert_eq!(counts.iter().sum::<usize>(), 24);
    }

    #[test]
    fn non_zero_segments_get_one_cell_when_grid_can_fit_all_segments() {
        let segments = vec![
            seg(ContextBudgetCategoryKind::SystemPrompt, 1_000),
            seg(ContextBudgetCategoryKind::ToolDefinitions, 1),
            seg(ContextBudgetCategoryKind::Messages, 1),
        ];

        let counts = allocate_heatmap_cells(&segments, 3);

        assert_eq!(
            counts,
            vec![1, 1, 1],
            "each non-zero segment should stay visible when the grid can fit every segment"
        );
    }

    #[test]
    fn heatmap_aggregates_into_context_budget_source_buckets() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "local/qwen3".to_string(),
            segments: vec![
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::AssistantMessage,
                    stack_order: 0,
                    estimated_tokens: 100,
                    label: "assistant".to_string(),
                },
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::UserMessage,
                    stack_order: 1,
                    estimated_tokens: 40,
                    label: "user".to_string(),
                },
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::AssistantMessage,
                    stack_order: 2,
                    estimated_tokens: 60,
                    label: "assistant".to_string(),
                },
                ContextBudgetSegmentPayload {
                    kind: SegmentKind::System,
                    stack_order: 3,
                    estimated_tokens: 20,
                    label: "system".to_string(),
                },
            ],
            total_estimated_tokens: 220,
            context_limit: Some(256_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 256_000,
                used: 220,
                percent: 0.1,
            },
        };

        let segments = aggregated_categories_for_heatmap(&snapshot);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.kind)
                .collect::<Vec<_>>(),
            vec![
                ContextBudgetCategoryKind::SystemPrompt,
                ContextBudgetCategoryKind::Messages,
            ]
        );
        assert_eq!(segments[0].estimated_tokens, 20);
        assert_eq!(segments[1].estimated_tokens, 200);
    }

    #[test]
    fn render_keeps_empty_capacity_cells_visible() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "local/qwen3".to_string(),
            segments: vec![ContextBudgetSegmentPayload {
                kind: SegmentKind::System,
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
        let mut buffer = Buffer::empty(Rect::new(0, 0, 20, 10));

        render_context_budget_heatmap(
            &mut buffer,
            Rect::new(0, 0, 20, 10),
            &snapshot,
            default_palette(),
        );

        let empty_color = context_budget_empty_color(&default_palette());
        let empty_cells = buffer
            .content()
            .iter()
            .filter(|cell| {
                is_context_budget_heatmap_cell(cell, default_palette()) && cell.fg == empty_color
            })
            .count();
        assert!(
            empty_cells > 0,
            "heatmap should paint remaining capacity cells explicitly instead of leaving blanks"
        );
    }

    #[test]
    fn heatmap_grid_uses_fixed_ten_by_ten_layout() {
        assert_eq!(heatmap_grid_columns(72), 10);
        assert_eq!(heatmap_grid_columns(20), 10);
        assert_eq!(heatmap_grid_rows(15), 10);
        assert_eq!(heatmap_grid_rows(10), 10);
    }

    #[test]
    fn heatmap_marks_cells_by_used_and_empty_symbols() {
        let palette = default_palette();
        let used_color =
            context_budget_color_for_category(ContextBudgetCategoryKind::Messages, &palette);
        let empty_color = context_budget_empty_color(&palette);

        let mut used = Cell::default();
        used.set_symbol(HEATMAP_FULL_SYMBOL);
        used.set_style(Style::new().fg(used_color));
        assert!(is_context_budget_heatmap_cell(&used, palette));

        let mut empty = Cell::default();
        empty.set_symbol(HEATMAP_EMPTY_SYMBOL);
        empty.set_style(Style::new().fg(empty_color));
        assert!(is_context_budget_heatmap_cell(&empty, palette));

        let plain = Cell::default();
        assert!(!is_context_budget_heatmap_cell(&plain, palette));
    }

    #[test]
    fn occupied_heatmap_cells_rounds_usage_against_fixed_hundred_cell_grid() {
        let snapshot = ContextBudgetSnapshotPayload {
            model_id: "local/qwen3".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 0,
            context_limit: Some(1_000),
            display: ContextBudgetDisplayPayload::Absolute {
                limit: 1_000,
                used: 421,
                percent: 42.1,
            },
        };

        assert_eq!(occupied_heatmap_cells(&snapshot, 100, 3), 42);
    }
}
