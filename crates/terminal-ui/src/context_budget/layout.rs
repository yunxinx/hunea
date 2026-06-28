use ratatui::layout::Rect;

pub(super) const CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING: u16 = 2;
pub(super) const CONTEXT_BUDGET_COLUMN_GAP: u16 = 3;
pub(super) const CONTEXT_BUDGET_HEATMAP_WIDTH: u16 = 20;
pub(super) const CONTEXT_BUDGET_MIN_HEATMAP_WIDTH: u16 = CONTEXT_BUDGET_HEATMAP_WIDTH;
pub(super) const CONTEXT_BUDGET_MIN_LEGEND_WIDTH: u16 = 20;
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

    let heatmap_width = CONTEXT_BUDGET_HEATMAP_WIDTH;
    let legend_width = content
        .width
        .saturating_sub(heatmap_width + CONTEXT_BUDGET_COLUMN_GAP)
        .clamp(
            CONTEXT_BUDGET_MIN_LEGEND_WIDTH,
            CONTEXT_BUDGET_MAX_LEGEND_WIDTH,
        );
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
        let body = Rect::new(0, 2, 45, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.content.x, 2);
        assert_eq!(layout.content.width, 43);
        assert_eq!(layout.heatmap.x, layout.content.x);
        assert_eq!(layout.heatmap.width, 20);
        assert_eq!(layout.heatmap.height, layout.content.height);
        assert_eq!(layout.legend.height, layout.content.height);
        assert_eq!(layout.legend.x, layout.heatmap.x + layout.heatmap.width + 3);
        assert!(
            layout.legend.width >= CONTEXT_BUDGET_MIN_LEGEND_WIDTH,
            "legend should keep enough room for visible natural-copy legend rows"
        );
    }

    #[test]
    fn body_layout_returns_none_when_too_narrow_for_two_columns() {
        let body = Rect::new(0, 0, 44, 10);

        assert_eq!(context_budget_body_layout(body), None);
    }
}
