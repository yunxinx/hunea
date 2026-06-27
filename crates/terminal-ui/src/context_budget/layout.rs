use ratatui::layout::Rect;

pub(super) const CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING: u16 = 2;
pub(super) const CONTEXT_BUDGET_MIN_BODY_HEIGHT_FOR_SPLIT: u16 = 3;
pub(super) const CONTEXT_BUDGET_MIN_HEATMAP_HEIGHT: u16 = 1;
pub(super) const CONTEXT_BUDGET_LEGEND_COLUMN_GAP: u16 = 4;
pub(super) const CONTEXT_BUDGET_MIN_LEGEND_COLUMN_WIDTH: u16 = 18;
pub(super) const CONTEXT_BUDGET_MIN_WIDTH_FOR_TWO_COLUMN_LEGEND: u16 =
    CONTEXT_BUDGET_MIN_LEGEND_COLUMN_WIDTH * 2 + CONTEXT_BUDGET_LEGEND_COLUMN_GAP;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContextBudgetBodyLayout {
    pub(super) content: Rect,
    pub(super) heatmap: Rect,
    pub(super) divider: Rect,
    pub(super) legend: Rect,
    pub(super) legend_columns: LegendColumns,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LegendColumns {
    pub(super) left: Rect,
    pub(super) right: Option<Rect>,
    pub(super) rows_per_column: usize,
}

pub(super) fn context_budget_body_layout(body: Rect) -> Option<ContextBudgetBodyLayout> {
    let content = inset_horizontal(body, CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING);
    if content.height < CONTEXT_BUDGET_MIN_BODY_HEIGHT_FOR_SPLIT {
        return None;
    }

    let available_height = content.height.saturating_sub(1);
    let min_heatmap_height =
        CONTEXT_BUDGET_MIN_HEATMAP_HEIGHT.min(available_height.saturating_sub(1).max(1));
    let heatmap_height = available_height.div_ceil(2).max(min_heatmap_height);
    let legend_height = content.height.saturating_sub(heatmap_height + 1);

    let heatmap = Rect::new(content.x, content.y, content.width, heatmap_height);
    let divider = Rect::new(content.x, content.y + heatmap_height, content.width, 1);
    let legend = Rect::new(
        content.x,
        divider.y + divider.height,
        content.width,
        legend_height,
    );

    Some(ContextBudgetBodyLayout {
        content,
        heatmap,
        divider,
        legend,
        legend_columns: legend_columns(legend),
    })
}

pub(super) fn legend_slot_for_rank(rank: usize, rows_per_column: usize) -> (usize, usize) {
    if rows_per_column == 0 {
        return (0, 0);
    }

    if rank < rows_per_column {
        (0, rank)
    } else {
        (1, rank - rows_per_column)
    }
}

fn legend_columns(area: Rect) -> LegendColumns {
    let rows_per_column = usize::from(area.height.max(1));
    if area.width < CONTEXT_BUDGET_MIN_WIDTH_FOR_TWO_COLUMN_LEGEND {
        return LegendColumns {
            left: area,
            right: None,
            rows_per_column,
        };
    }

    let gap = CONTEXT_BUDGET_LEGEND_COLUMN_GAP.min(area.width.saturating_sub(1));
    let left_width = area.width.saturating_sub(gap) / 2;
    let right_width = area.width.saturating_sub(left_width + gap);
    if left_width < CONTEXT_BUDGET_MIN_LEGEND_COLUMN_WIDTH
        || right_width < CONTEXT_BUDGET_MIN_LEGEND_COLUMN_WIDTH
    {
        return LegendColumns {
            left: area,
            right: None,
            rows_per_column,
        };
    }

    LegendColumns {
        left: Rect::new(area.x, area.y, left_width, area.height),
        right: Some(Rect::new(
            area.x + left_width + gap,
            area.y,
            right_width,
            area.height,
        )),
        rows_per_column,
    }
}

fn inset_horizontal(area: Rect, inset: u16) -> Rect {
    if area.width <= inset.saturating_mul(2) {
        return area;
    }

    Rect::new(
        area.x + inset,
        area.y,
        area.width.saturating_sub(inset.saturating_mul(2)),
        area.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_layout_reserves_divider_and_shared_width() {
        let body = Rect::new(0, 2, 80, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.content.x, 2);
        assert_eq!(layout.content.width, 76);
        assert_eq!(layout.heatmap.x, layout.content.x);
        assert_eq!(layout.divider.x, layout.content.x);
        assert_eq!(layout.legend.x, layout.content.x);
        assert_eq!(layout.heatmap.width, layout.content.width);
        assert_eq!(layout.divider.width, layout.content.width);
        assert_eq!(layout.legend.width, layout.content.width);
        assert_eq!(layout.divider.height, 1);
        assert_eq!(
            layout.heatmap.height + layout.legend.height + layout.divider.height,
            10
        );
    }

    #[test]
    fn body_layout_falls_back_to_single_column_when_too_narrow() {
        let body = Rect::new(0, 0, 32, 8);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.legend_columns.left, layout.legend);
        assert_eq!(layout.legend_columns.right, None);
    }

    #[test]
    fn legend_slot_uses_column_major_left_then_right() {
        assert_eq!(legend_slot_for_rank(0, 3), (0, 0));
        assert_eq!(legend_slot_for_rank(2, 3), (0, 2));
        assert_eq!(legend_slot_for_rank(3, 3), (1, 0));
        assert_eq!(legend_slot_for_rank(5, 3), (1, 2));
    }
}
