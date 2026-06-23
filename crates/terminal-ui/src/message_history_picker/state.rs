use session_store::MessageHistoryRow;

use crate::list_selection::{ListNavigationDirection, PagedSelection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageHistoryPickerState {
    pub(super) rows: Vec<MessageHistoryRow>,
    pub(super) selected: usize,
    pub(super) opened_at_ms: i64,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
}

impl Default for MessageHistoryPickerState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            opened_at_ms: 0,
            is_loading: true,
            error: None,
        }
    }
}

impl MessageHistoryPickerState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.rows.len())
    }

    pub(super) fn select_latest_row(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.selected = self.selection().move_selection(direction);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.selected = self.selection().move_page(direction, page_size);
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        self.selection().page_start(page_size)
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> {
        self.selection().page_indices(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        self.selection().page_number(page_size)
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        self.selection().page_count(page_size)
    }

    pub(super) fn selected_position_label(&self) -> usize {
        self.selection().selected_position_label()
    }

    pub(super) fn selected_row(&self) -> Option<&MessageHistoryRow> {
        self.rows.get(self.selected)
    }
}
