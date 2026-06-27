use ratatui::layout::Rect;

pub(super) const CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING: u16 = 2;
pub(super) const CONTEXT_BUDGET_COLUMN_GAP: u16 = 3;
pub(super) const CONTEXT_BUDGET_MIN_HEATMAP_WIDTH: u16 = 18;
pub(super) const CONTEXT_BUDGET_MIN_LEGEND_WIDTH: u16 = 22;
pub(super) const CONTEXT_BUDGET_MAX_LEGEND_WIDTH: u16 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContextBudgetBodyLayout {
    pub(super) content: Rect,
    pub(super) heatmap: Rect,
    pub(super) legend: Rect,
}

pub(super) fn context_budget_body_layout(body: Rect) -> Option<ContextBudgetBodyLayout> {
    let content = inset_horizontal(body, CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING);
    if content.height == 0 {
        return None;
    }

    let min_total_width = CONTEXT_BUDGET_MIN_HEATMAP_WIDTH
        + CONTEXT_BUDGET_COLUMN_GAP
        + CONTEXT_BUDGET_MIN_LEGEND_WIDTH;
    if content.width < min_total_width {
        return None;
    }

    let max_legend_width = content
        .width
        .saturating_sub(CONTEXT_BUDGET_MIN_HEATMAP_WIDTH + CONTEXT_BUDGET_COLUMN_GAP)
        .min(CONTEXT_BUDGET_MAX_LEGEND_WIDTH);
    let preferred_legend_width = content.width.div_ceil(2);
    let legend_width = preferred_legend_width
        .max(CONTEXT_BUDGET_MIN_LEGEND_WIDTH)
        .min(max_legend_width);
    let heatmap_width = content
        .width
        .saturating_sub(legend_width + CONTEXT_BUDGET_COLUMN_GAP);
    let heatmap = Rect::new(content.x, content.y, heatmap_width, content.height);
    let legend = Rect::new(
        heatmap.x + heatmap.width + CONTEXT_BUDGET_COLUMN_GAP,
        content.y,
        legend_width,
        content.height,
    );

    Some(ContextBudgetBodyLayout {
        content,
        heatmap,
        legend,
    })
}

fn inset_horizontal(area: Rect, inset: u16) -> Rect {
    if area.width <= inset {
        return area;
    }

    Rect::new(
        area.x + inset,
        area.y,
        area.width.saturating_sub(inset),
        area.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_layout_uses_right_side_legend_column() {
        let body = Rect::new(0, 2, 80, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.content.x, 2);
        assert_eq!(layout.content.width, 78);
        assert_eq!(layout.heatmap.x, layout.content.x);
        assert_eq!(layout.heatmap.height, layout.content.height);
        assert_eq!(layout.legend.height, layout.content.height);
        assert_eq!(layout.legend.x, layout.heatmap.x + layout.heatmap.width + 3);
        assert_eq!(
            layout.heatmap.width + layout.legend.width + 3,
            layout.content.width
        );
        assert!(
            layout.heatmap.width <= layout.content.width.div_ceil(2),
            "heatmap should stay within roughly half of the panel width"
        );
    }

    #[test]
    fn body_layout_returns_none_when_too_narrow_for_two_columns() {
        let body = Rect::new(0, 0, 40, 8);

        assert_eq!(context_budget_body_layout(body), None);
    }
}
