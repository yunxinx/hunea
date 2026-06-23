use std::ops::Range;

/// 单步列表导航方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListNavigationDirection {
    Previous,
    Next,
}

impl ListNavigationDirection {
    pub(crate) const fn from_delta(delta: isize) -> Option<Self> {
        if delta < 0 {
            Some(Self::Previous)
        } else if delta > 0 {
            Some(Self::Next)
        } else {
            None
        }
    }
}

/// 基于 `selected + item_count` 的分页选择计算。
///
/// 它不持有行数据，只负责所有全屏列表共享的索引、分页与可见行换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PagedSelection {
    selected: usize,
    item_count: usize,
}

/// 基于 `selected + item_count` 的固定窗口选择计算。
///
/// 它用于弹窗内的滚动列表：选中项连续移动，`scroll_start` 只负责让选中项留在可见窗口内。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisibleWindowSelection {
    selected: usize,
    item_count: usize,
}

impl PagedSelection {
    pub(crate) fn new(selected: usize, item_count: usize) -> Self {
        Self {
            selected: selected.min(item_count.saturating_sub(1)),
            item_count,
        }
    }

    pub(crate) fn move_selection(self, direction: ListNavigationDirection) -> usize {
        if self.item_count == 0 {
            return 0;
        }
        let last = self.item_count.saturating_sub(1);
        match direction {
            ListNavigationDirection::Previous => self.selected.saturating_sub(1),
            ListNavigationDirection::Next => self.selected.saturating_add(1).min(last),
        }
    }

    pub(crate) fn move_page(self, direction: ListNavigationDirection, page_size: usize) -> usize {
        if self.item_count == 0 {
            return 0;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.item_count.saturating_sub(1) / page_size;
        let next_page = match direction {
            ListNavigationDirection::Previous => current_page.saturating_sub(1),
            ListNavigationDirection::Next => current_page.saturating_add(1).min(last_page),
        };
        (next_page * page_size).min(self.item_count.saturating_sub(1))
    }

    pub(crate) fn page_start(self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    pub(crate) fn page_indices(self, page_size: usize) -> Range<usize> {
        let page_size = page_size.max(1);
        let start = self.page_start(page_size).min(self.item_count);
        let end = start.saturating_add(page_size).min(self.item_count);
        start..end
    }

    pub(crate) fn page_number(self, page_size: usize) -> usize {
        if self.item_count == 0 {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    pub(crate) fn page_count(self, page_size: usize) -> usize {
        if self.item_count == 0 {
            return 1;
        }
        self.item_count.saturating_sub(1) / page_size.max(1) + 1
    }

    pub(crate) fn selected_position_label(self) -> usize {
        if self.item_count == 0 {
            0
        } else {
            self.selected + 1
        }
    }

    pub(crate) fn select_visible_index(
        self,
        page_size: usize,
        visible_offset: usize,
    ) -> Option<usize> {
        let index = self.page_start(page_size).saturating_add(visible_offset);
        (index < self.item_count).then_some(index)
    }
}

impl VisibleWindowSelection {
    pub(crate) fn new(selected: usize, item_count: usize) -> Self {
        Self {
            selected: selected.min(item_count.saturating_sub(1)),
            item_count,
        }
    }

    pub(crate) fn move_selection(self, direction: ListNavigationDirection) -> usize {
        PagedSelection::new(self.selected, self.item_count).move_selection(direction)
    }

    pub(crate) fn scroll_start_for_selection(
        self,
        current_scroll_start: usize,
        visible_rows: usize,
    ) -> usize {
        if self.item_count == 0 {
            return 0;
        }
        let visible_rows = visible_rows.max(1);
        let max_scroll_start = self.item_count.saturating_sub(visible_rows);
        let mut scroll_start = current_scroll_start.min(max_scroll_start);
        if self.selected < scroll_start {
            scroll_start = self.selected;
        }
        if self.selected >= scroll_start.saturating_add(visible_rows) {
            scroll_start = self.selected + 1 - visible_rows;
        }
        scroll_start.min(max_scroll_start)
    }

    pub(crate) fn select_visible_index(
        self,
        scroll_start: usize,
        visible_offset: usize,
    ) -> Option<usize> {
        let index = scroll_start.saturating_add(visible_offset);
        (index < self.item_count).then_some(index)
    }
}

/// 在带稳定 `row_id` 的列表中查找目标行索引。
pub(crate) fn row_index_by_id<T>(
    rows: &[T],
    row_id: Option<&str>,
    row_id_for: impl Fn(&T) -> &str,
) -> Option<usize> {
    let row_id = row_id?;
    rows.iter().position(|row| row_id_for(row) == row_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_has_neutral_position_and_single_page() {
        let selection = PagedSelection::new(5, 0);

        assert_eq!(selection.selected_position_label(), 0);
        assert_eq!(selection.page_start(3), 0);
        assert_eq!(selection.page_indices(3), 0..0);
        assert_eq!(selection.page_number(3), 1);
        assert_eq!(selection.page_count(3), 1);
        assert_eq!(selection.select_visible_index(3, 0), None);
        assert_eq!(selection.move_selection(ListNavigationDirection::Next), 0);
    }

    #[test]
    fn selection_and_page_moves_saturate_at_bounds() {
        let middle = PagedSelection::new(3, 7);
        let first = PagedSelection::new(0, 7);
        let last = PagedSelection::new(6, 7);

        assert_eq!(middle.move_selection(ListNavigationDirection::Previous), 2);
        assert_eq!(middle.move_selection(ListNavigationDirection::Next), 4);
        assert_eq!(first.move_selection(ListNavigationDirection::Previous), 0);
        assert_eq!(last.move_selection(ListNavigationDirection::Next), 6);

        assert_eq!(
            PagedSelection::new(5, 7).move_page(ListNavigationDirection::Previous, 3),
            0
        );
        assert_eq!(
            PagedSelection::new(3, 7).move_page(ListNavigationDirection::Next, 3),
            6
        );
        assert_eq!(last.move_page(ListNavigationDirection::Next, 3), 6);
    }

    #[test]
    fn page_metadata_uses_current_page_and_one_based_labels() {
        let selection = PagedSelection::new(4, 10);
        let clamped_selection = PagedSelection::new(12, 10);

        assert_eq!(selection.page_start(3), 3);
        assert_eq!(selection.page_indices(3), 3..6);
        assert_eq!(selection.page_number(3), 2);
        assert_eq!(selection.page_count(3), 4);
        assert_eq!(selection.selected_position_label(), 5);
        assert_eq!(clamped_selection.page_number(3), 4);
        assert_eq!(clamped_selection.selected_position_label(), 10);
    }

    #[test]
    fn visible_index_is_resolved_inside_current_page() {
        let selection = PagedSelection::new(4, 8);

        assert_eq!(selection.select_visible_index(3, 0), Some(3));
        assert_eq!(selection.select_visible_index(3, 2), Some(5));
        assert_eq!(selection.select_visible_index(3, 5), None);
    }

    #[test]
    fn row_index_by_id_resolves_optional_identity() {
        struct Row {
            row_id: &'static str,
        }
        let rows = [Row { row_id: "first" }, Row { row_id: "second" }];

        assert_eq!(
            row_index_by_id(&rows, Some("second"), |row| row.row_id),
            Some(1)
        );
        assert_eq!(
            row_index_by_id(&rows, Some("missing"), |row| row.row_id),
            None
        );
        assert_eq!(row_index_by_id(&rows, None, |row| row.row_id), None);
    }

    #[test]
    fn navigation_direction_maps_only_non_zero_deltas() {
        assert_eq!(
            ListNavigationDirection::from_delta(-4),
            Some(ListNavigationDirection::Previous)
        );
        assert_eq!(
            ListNavigationDirection::from_delta(7),
            Some(ListNavigationDirection::Next)
        );
        assert_eq!(ListNavigationDirection::from_delta(0), None);
    }

    #[test]
    fn visible_window_selection_keeps_selected_item_in_view() {
        let selection = VisibleWindowSelection::new(5, 10);

        assert_eq!(selection.scroll_start_for_selection(0, 3), 3);
        assert_eq!(selection.scroll_start_for_selection(4, 3), 4);
        assert_eq!(selection.scroll_start_for_selection(9, 3), 5);
        assert_eq!(
            VisibleWindowSelection::new(1, 10).scroll_start_for_selection(4, 3),
            1
        );
    }

    #[test]
    fn visible_window_selection_maps_visible_offsets_from_scroll_start() {
        let selection = VisibleWindowSelection::new(4, 6);

        assert_eq!(selection.select_visible_index(3, 0), Some(3));
        assert_eq!(selection.select_visible_index(3, 2), Some(5));
        assert_eq!(selection.select_visible_index(3, 3), None);
        assert_eq!(
            VisibleWindowSelection::new(0, 0).scroll_start_for_selection(3, 2),
            0
        );
    }
}
