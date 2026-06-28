use ratatui::layout::{Constraint, Layout, Rect};

pub(super) const CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING: u16 = 2;
pub(super) const CONTEXT_BUDGET_COLUMN_GAP: u16 = 3;
pub(super) const CONTEXT_BUDGET_HEATMAP_WIDTH: u16 = 20;
pub(super) const CONTEXT_BUDGET_MIN_HEATMAP_WIDTH: u16 = CONTEXT_BUDGET_HEATMAP_WIDTH;
pub(super) const CONTEXT_BUDGET_MIN_LEGEND_WIDTH: u16 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContextBudgetBodyLayout {
    pub(super) content: Rect,
    pub(super) heatmap: Rect,
    pub(super) legend: Rect,
}

pub(super) fn context_budget_body_layout(body: Rect) -> Option<ContextBudgetBodyLayout> {
    let [_, content] = Layout::horizontal([
        Constraint::Length(CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING),
        Constraint::Fill(1),
    ])
    .areas(body);
    if content.height == 0 {
        return None;
    }

    let min_total_width = CONTEXT_BUDGET_MIN_HEATMAP_WIDTH
        + CONTEXT_BUDGET_COLUMN_GAP
        + CONTEXT_BUDGET_MIN_LEGEND_WIDTH;
    if content.width < min_total_width {
        return None;
    }

    let [heatmap, _, legend] = Layout::horizontal([
        Constraint::Length(CONTEXT_BUDGET_HEATMAP_WIDTH),
        Constraint::Length(CONTEXT_BUDGET_COLUMN_GAP),
        Constraint::Fill(1),
    ])
    .areas(content);

    Some(ContextBudgetBodyLayout {
        content,
        heatmap,
        legend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_layout_uses_declared_left_padding() {
        let body = Rect::new(5, 2, 72, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(
            layout.content.x,
            body.x + CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING,
            "content should start after the declared left padding"
        );
        assert_eq!(
            layout.content.width,
            body.width
                .saturating_sub(CONTEXT_BUDGET_BODY_HORIZONTAL_PADDING),
            "content width should drop only the declared left padding"
        );
    }

    #[test]
    fn body_layout_uses_right_side_legend_column() {
        let body = Rect::new(0, 2, 72, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.content.x, 2);
        assert_eq!(layout.content.width, 70);
        assert_eq!(layout.heatmap.x, layout.content.x);
        assert_eq!(layout.heatmap.width, 20);
        assert_eq!(layout.heatmap.height, layout.content.height);
        assert_eq!(layout.legend.height, layout.content.height);
        assert_eq!(layout.legend.x, layout.heatmap.x + layout.heatmap.width + 3);
        assert_eq!(
            layout.legend.width + layout.heatmap.width + 3,
            layout.content.width,
            "legend should consume all remaining width after the fixed heatmap column"
        );
        assert!(
            layout.legend.width > CONTEXT_BUDGET_MIN_LEGEND_WIDTH,
            "legend should expand into the remaining width instead of stopping at the minimum"
        );
    }

    #[test]
    fn body_layout_keeps_heatmap_gap_and_legend_in_one_horizontal_split() {
        let body = Rect::new(0, 2, 96, 10);

        let layout = context_budget_body_layout(body).expect("layout should exist");

        assert_eq!(layout.heatmap.width, CONTEXT_BUDGET_HEATMAP_WIDTH);
        assert_eq!(
            layout.legend.x,
            layout.heatmap.right() + CONTEXT_BUDGET_COLUMN_GAP,
            "legend should begin exactly after the fixed gap"
        );
        assert_eq!(
            layout.legend.right(),
            layout.content.right(),
            "legend should consume the remaining horizontal space"
        );
    }

    #[test]
    fn body_layout_returns_none_when_too_narrow_for_two_columns() {
        let body = Rect::new(0, 0, 44, 10);

        assert_eq!(context_budget_body_layout(body), None);
    }
}
