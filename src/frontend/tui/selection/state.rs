use std::time::{Duration, Instant};

/// `SELECTION_MULTI_CLICK_WINDOW` 表示双击/三击识别窗口。
pub(crate) const SELECTION_MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// `SELECTION_AUTO_SCROLL_INTERVAL` 表示拖拽选区贴边后的自动滚动节奏。
pub(crate) const SELECTION_AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// `SelectionPoint` 用统一文档坐标记录选区端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionPoint {
    line: usize,
    column: usize,
}

impl SelectionPoint {
    pub(crate) const fn new(line: usize, column: usize) -> Self {
        Self { line, column }
    }

    pub(crate) const fn line(self) -> usize {
        self.line
    }

    pub(crate) const fn column(self) -> usize {
        self.column
    }
}

/// `MousePosition` 记录最近一次拖拽时的鼠标位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MousePosition {
    column: u16,
    row: u16,
}

impl MousePosition {
    pub(crate) const fn new(column: u16, row: u16) -> Self {
        Self { column, row }
    }

    pub(crate) const fn column(self) -> u16 {
        self.column
    }

    pub(crate) const fn row(self) -> u16 {
        self.row
    }
}

/// `SelectionState` 保存当前屏幕选区状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionState {
    active: bool,
    dragging: bool,
    anchor: SelectionPoint,
    focus: SelectionPoint,
}

impl SelectionState {
    pub(crate) const fn is_active(self) -> bool {
        self.active
    }

    pub(crate) const fn is_dragging(self) -> bool {
        self.dragging
    }

    #[cfg(test)]
    pub(crate) const fn anchor(self) -> SelectionPoint {
        self.anchor
    }

    pub(crate) const fn focus(self) -> SelectionPoint {
        self.focus
    }

    pub(crate) fn begin(&mut self, point: SelectionPoint) {
        self.active = true;
        self.dragging = true;
        self.anchor = point;
        self.focus = point;
    }

    pub(crate) fn update_focus(&mut self, point: SelectionPoint) {
        self.focus = point;
    }

    pub(crate) fn finish(&mut self, point: SelectionPoint) {
        self.focus = point;
        self.dragging = false;
        self.active = true;
    }

    pub(crate) fn stop_drag(&mut self) {
        self.dragging = false;
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn select_range(&mut self, anchor: SelectionPoint, focus: SelectionPoint) {
        self.active = true;
        self.dragging = false;
        self.anchor = anchor;
        self.focus = focus;
    }

    pub(crate) fn ordered_points(self) -> Option<(SelectionPoint, SelectionPoint)> {
        if !self.active {
            return None;
        }

        let mut start = self.anchor;
        let mut end = self.focus;
        if end.line < start.line || (end.line == start.line && end.column < start.column) {
            std::mem::swap(&mut start, &mut end);
        }
        if start == end {
            return None;
        }

        Some((start, end))
    }
}

/// `SelectionClickState` 服务双击/三击扩选。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionClickState {
    point: SelectionPoint,
    count: u8,
    at: Option<Instant>,
}

impl SelectionClickState {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn register(&mut self, point: SelectionPoint, at: Instant) -> u8 {
        let mut next_count = 1;
        if let Some(previous_at) = self.at
            && at.duration_since(previous_at) <= SELECTION_MULTI_CLICK_WINDOW
            && self.point.line == point.line
            && self.point.column.abs_diff(point.column) <= 1
        {
            next_count = self.count.saturating_add(1);
            if next_count > 3 {
                next_count = 1;
            }
        }

        self.point = point;
        self.count = next_count;
        self.at = Some(at);
        next_count
    }
}

/// `AutoScrollDirection` 表示拖拽选区时的自动滚动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AutoScrollDirection {
    #[default]
    None,
    Down,
    Up,
}

pub(crate) fn selection_auto_scroll_direction_for_mouse_row(
    row: u16,
    viewport_height: usize,
) -> AutoScrollDirection {
    if viewport_height == 0 {
        return AutoScrollDirection::None;
    }

    if row == 0 {
        return AutoScrollDirection::Up;
    }
    if usize::from(row) >= viewport_height.saturating_sub(1) {
        return AutoScrollDirection::Down;
    }

    AutoScrollDirection::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_state_transition_helpers_keep_drag_lifecycle_consistent() {
        let anchor = SelectionPoint::new(2, 3);
        let focus = SelectionPoint::new(4, 6);
        let mut selection = SelectionState::default();

        selection.begin(anchor);
        assert!(selection.is_active());
        assert!(selection.is_dragging());
        assert_eq!(selection.anchor(), anchor);
        assert_eq!(selection.focus(), anchor);

        selection.update_focus(focus);
        selection.finish(focus);
        assert!(selection.is_active());
        assert!(!selection.is_dragging());
        assert_eq!(selection.focus(), focus);
        assert_eq!(selection.ordered_points(), Some((anchor, focus)));

        selection.clear();
        assert_eq!(selection, SelectionState::default());
    }

    #[test]
    fn selection_click_state_register_cycles_after_triple_click() {
        let point = SelectionPoint::new(3, 5);
        let start = Instant::now();
        let mut click = SelectionClickState::default();

        assert_eq!(click.register(point, start), 1);
        assert_eq!(click.register(point, start + Duration::from_millis(100)), 2);
        assert_eq!(click.register(point, start + Duration::from_millis(200)), 3);
        assert_eq!(click.register(point, start + Duration::from_millis(300)), 1);

        click.clear();
        assert_eq!(click, SelectionClickState::default());
    }

    #[test]
    fn ordered_points_normalizes_reverse_drag() {
        let selection = SelectionState {
            active: true,
            dragging: false,
            anchor: SelectionPoint { line: 4, column: 7 },
            focus: SelectionPoint { line: 2, column: 3 },
        };

        let (start, end) = selection
            .ordered_points()
            .expect("active multi-cell selection should normalize");
        assert_eq!(start, SelectionPoint { line: 2, column: 3 });
        assert_eq!(end, SelectionPoint { line: 4, column: 7 });
    }

    #[test]
    fn auto_scroll_direction_triggers_on_first_and_last_visible_rows() {
        assert_eq!(
            selection_auto_scroll_direction_for_mouse_row(0, 4),
            AutoScrollDirection::Up
        );
        assert_eq!(
            selection_auto_scroll_direction_for_mouse_row(3, 4),
            AutoScrollDirection::Down
        );
        assert_eq!(
            selection_auto_scroll_direction_for_mouse_row(1, 4),
            AutoScrollDirection::None
        );
    }
}
