//! Heatmap cell allocation for context budget segments.

use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use runtime_domain::context_budget::ContextBudgetSnapshot;

use super::{
    CONTEXT_BUDGET_HEATMAP_CELL_WIDTH, CONTEXT_BUDGET_HEATMAP_GRID_COLUMNS,
    CONTEXT_BUDGET_HEATMAP_GRID_ROWS, blank_line,
    segment_colors::context_budget_color_for_category,
    summary::{ContextBudgetCategoryKind, aggregated_category_totals},
};
use crate::theme::{TerminalPalette, context_budget_empty_color};

const HEATMAP_FULL_SYMBOL: &str = "◼";
const HEATMAP_EMPTY_SYMBOL: &str = "⛶";

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
    debug_assert!(
        total_tokens > 0,
        "allocate_heatmap_cells expects at least one non-zero category"
    );

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

pub(super) fn build_context_budget_heatmap_lines(
    area: Rect,
    snapshot: &ContextBudgetSnapshot,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }

    let grid_columns = heatmap_grid_columns(area.width);
    let grid_rows = heatmap_grid_rows(area.height);
    let total_cells = grid_columns.saturating_mul(grid_rows);
    if total_cells == 0 {
        return blank_heatmap_lines(area);
    }

    let segments = aggregated_categories_for_heatmap(snapshot);
    let occupied_cells = occupied_heatmap_cells(snapshot, total_cells, segments.len());
    let counts = allocate_heatmap_cells(&segments, occupied_cells);
    let fill_kinds = build_heatmap_fill_kinds(total_cells, &segments, &counts);
    let row_width = usize::from(area.width);
    let row_count = usize::from(area.height);
    let empty_color = context_budget_empty_color(&palette);

    (0..row_count)
        .map(|row| {
            let mut spans = Vec::with_capacity(grid_columns.saturating_add(1));
            let mut rendered_width = 0usize;

            for column in 0..grid_columns {
                let remaining_width = row_width.saturating_sub(rendered_width);
                if remaining_width == 0 {
                    break;
                }

                let cell_index = row.saturating_mul(grid_columns).saturating_add(column);
                let fill = fill_kinds
                    .get(cell_index)
                    .copied()
                    .unwrap_or(HeatmapCellFill { kind: None });
                let color = fill
                    .kind
                    .map(|kind| context_budget_color_for_category(kind, &palette))
                    .unwrap_or(empty_color);
                let style = Style::new().fg(color);
                let cell_text = if remaining_width == 1 {
                    heatmap_symbol(fill).to_string()
                } else {
                    format!("{} ", heatmap_symbol(fill))
                };
                rendered_width = rendered_width.saturating_add(if remaining_width == 1 {
                    1
                } else {
                    CONTEXT_BUDGET_HEATMAP_CELL_WIDTH
                });
                spans.push(Span::styled(cell_text, style));
            }

            if rendered_width < row_width {
                spans.push(Span::raw(" ".repeat(row_width - rendered_width)));
            }

            Line::from(spans)
        })
        .collect()
}

fn build_heatmap_fill_kinds(
    total_cells: usize,
    segments: &[HeatmapCategory],
    counts: &[usize],
) -> Vec<HeatmapCellFill> {
    let mut fill_kinds = Vec::with_capacity(total_cells);
    for (segment, count) in segments.iter().zip(counts.iter()) {
        for _ in 0..*count {
            fill_kinds.push(HeatmapCellFill {
                kind: Some(segment.kind),
            });
        }
    }
    fill_kinds.resize(total_cells, HeatmapCellFill { kind: None });
    fill_kinds
}

fn blank_heatmap_lines(area: Rect) -> Vec<Line<'static>> {
    (0..area.height)
        .map(|_| blank_line(usize::from(area.width)))
        .collect()
}

pub(super) fn heatmap_grid_columns(width: u16) -> usize {
    usize::from(width.max(1))
        .div_ceil(CONTEXT_BUDGET_HEATMAP_CELL_WIDTH)
        .min(CONTEXT_BUDGET_HEATMAP_GRID_COLUMNS)
}

fn heatmap_grid_rows(height: u16) -> usize {
    usize::from(height).min(CONTEXT_BUDGET_HEATMAP_GRID_ROWS)
}

pub(super) fn occupied_heatmap_cells(
    snapshot: &ContextBudgetSnapshot,
    total_cells: usize,
    segment_count: usize,
) -> usize {
    let limit = snapshot.usage.limit.get();
    let used = snapshot.usage.used;
    if total_cells == 0 || used == 0 {
        return 0;
    }
    let exact = (used as f64 / limit as f64) * total_cells as f64;
    let occupied = exact.round() as usize;
    occupied.clamp(segment_count.min(total_cells), total_cells)
}

pub(super) fn aggregated_categories_for_heatmap(
    snapshot: &ContextBudgetSnapshot,
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

#[cfg(test)]
use ratatui::buffer::Cell;

#[cfg(test)]
#[allow(dead_code)]
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
    use crate::context_budget::CONTEXT_BUDGET_HEATMAP_WIDTH;
    use runtime_domain::context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
    };

    use crate::theme::default_palette;

    fn seg(kind: ContextBudgetCategoryKind, tokens: usize) -> HeatmapCategory {
        HeatmapCategory {
            kind,
            estimated_tokens: tokens,
        }
    }

    fn limit(value: u32) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
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
        let snapshot = ContextBudgetSnapshot {
            model_id: "local/qwen3".to_string(),
            segments: vec![
                ContextSegment {
                    kind: SegmentKind::AssistantMessage,
                    estimated_tokens: 100,
                },
                ContextSegment {
                    kind: SegmentKind::UserMessage,
                    estimated_tokens: 40,
                },
                ContextSegment {
                    kind: SegmentKind::AssistantMessage,
                    estimated_tokens: 60,
                },
                ContextSegment {
                    kind: SegmentKind::System,
                    estimated_tokens: 20,
                },
            ],
            total_estimated_tokens: 220,
            usage: ContextWindowUsage {
                limit: limit(256_000),
                used: 220,
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
    fn build_heatmap_lines_fill_requested_area_width() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "local/qwen3".to_string(),
            segments: vec![ContextSegment {
                kind: SegmentKind::System,
                estimated_tokens: 10,
            }],
            total_estimated_tokens: 10,
            usage: ContextWindowUsage {
                limit: limit(80),
                used: 10,
            },
        };

        let lines = build_context_budget_heatmap_lines(
            Rect::new(0, 0, CONTEXT_BUDGET_HEATMAP_WIDTH, 3),
            &snapshot,
            default_palette(),
        );

        assert_eq!(lines.len(), 3);
        assert!(
            lines
                .iter()
                .all(|line| line.width() == usize::from(CONTEXT_BUDGET_HEATMAP_WIDTH))
        );
    }

    #[test]
    fn heatmap_grid_uses_fixed_ten_by_ten_layout() {
        assert_eq!(
            heatmap_grid_columns(72),
            CONTEXT_BUDGET_HEATMAP_GRID_COLUMNS
        );
        assert_eq!(
            heatmap_grid_columns(CONTEXT_BUDGET_HEATMAP_WIDTH),
            CONTEXT_BUDGET_HEATMAP_GRID_COLUMNS
        );
        assert_eq!(heatmap_grid_rows(15), CONTEXT_BUDGET_HEATMAP_GRID_ROWS);
        assert_eq!(
            heatmap_grid_rows(CONTEXT_BUDGET_HEATMAP_GRID_ROWS as u16),
            CONTEXT_BUDGET_HEATMAP_GRID_ROWS
        );
    }

    #[test]
    fn occupied_heatmap_cells_rounds_usage_against_fixed_hundred_cell_grid() {
        let snapshot = ContextBudgetSnapshot {
            model_id: "local/qwen3".to_string(),
            segments: Vec::new(),
            total_estimated_tokens: 0,
            usage: ContextWindowUsage {
                limit: limit(1_000),
                used: 421,
            },
        };

        assert_eq!(occupied_heatmap_cells(&snapshot, 100, 3), 42);
    }
}
