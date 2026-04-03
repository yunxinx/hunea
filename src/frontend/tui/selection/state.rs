use std::time::{Duration, Instant};

/// `SELECTION_MULTI_CLICK_WINDOW` 表示双击/三击识别窗口。
pub(crate) const SELECTION_MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// `SELECTION_AUTO_SCROLL_INTERVAL` 表示拖拽选区贴边后的自动滚动节奏。
pub(crate) const SELECTION_AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// `SelectionPoint` 用统一文档坐标记录选区端点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionPoint {
    pub(crate) line: usize,
    pub(crate) column: usize,
}

/// `MousePosition` 记录最近一次拖拽时的鼠标位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MousePosition {
    pub(crate) column: u16,
    pub(crate) row: u16,
}

/// `SelectionState` 保存当前屏幕选区状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectionState {
    pub(crate) active: bool,
    pub(crate) dragging: bool,
    pub(crate) anchor: SelectionPoint,
    pub(crate) focus: SelectionPoint,
}

impl SelectionState {
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
    pub(crate) point: SelectionPoint,
    pub(crate) count: u8,
    pub(crate) at: Option<Instant>,
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
