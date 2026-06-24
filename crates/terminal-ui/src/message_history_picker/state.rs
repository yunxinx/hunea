use runtime_domain::session::{MessageHistoryRow, SessionLoadRequestId};

use crate::{
    list_selection::{ListNavigationDirection, PagedSelection},
    text_search::CaseInsensitiveQuery,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageHistoryPickerState {
    pub(super) rows: Vec<MessageHistoryRow>,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) selected: usize,
    pub(super) selected_row_id: Option<i64>,
    pub(super) opened_at_ms: i64,
    pub(super) pending_request_id: Option<SessionLoadRequestId>,
    pub(super) search_query: String,
    pub(super) is_searching: bool,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
    pub(super) preview: Option<MessageHistoryPickerPreviewState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MessageHistoryPickerPreviewState {
    pub(super) row_index: usize,
    pub(super) scroll_offset: usize,
}

impl Default for MessageHistoryPickerState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            selected_row_id: None,
            opened_at_ms: 0,
            pending_request_id: None,
            search_query: String::new(),
            is_searching: false,
            is_loading: true,
            error: None,
            preview: None,
        }
    }
}

impl MessageHistoryPickerState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.filtered_indices.len())
    }

    pub(super) fn apply_filter(&mut self) {
        let query = self.search_query.trim();
        let query = CaseInsensitiveQuery::new(query);
        let selected_row_id = self
            .selected_row_id
            .or_else(|| self.selected_row().map(|row| row.id));
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| query.matches(&row.text).then_some(index))
            .collect();
        self.restore_selected_row_or_clamp(selected_row_id);
    }

    pub(super) fn select_latest_row(&mut self) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_row_id = None;
            return;
        }
        self.selected = self.filtered_indices.len().saturating_sub(1);
        self.sync_selected_row_id();
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_row_id = None;
            return;
        }
        self.selected = self.selection().move_selection(direction);
        self.sync_selected_row_id();
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_row_id = None;
            return;
        }
        self.selected = self.selection().move_page(direction, page_size);
        self.sync_selected_row_id();
    }

    pub(super) fn push_search_character(&mut self, character: char) {
        self.search_query.push(character);
        self.apply_filter();
    }

    pub(super) fn backspace_search(&mut self) {
        if self.search_query.pop().is_some() {
            self.apply_filter();
        }
    }

    pub(super) fn clear_search(&mut self) -> bool {
        if self.search_query.is_empty() && !self.is_searching {
            return false;
        }
        let selected_row_id = self.selected_row().map(|row| row.id);
        self.search_query.clear();
        self.apply_filter();
        self.select_row_id_or_clamp(selected_row_id);
        true
    }

    pub(super) fn exit_search(&mut self) -> bool {
        if !self.is_searching && self.search_query.is_empty() {
            return false;
        }
        let selected_row_id = self.selected_row().map(|row| row.id);
        let had_query = !self.search_query.is_empty();
        self.search_query.clear();
        self.is_searching = false;
        if had_query {
            self.apply_filter();
            self.select_row_id_or_clamp(selected_row_id);
        }
        true
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        self.selection().page_start(page_size)
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> {
        self.filtered_indices
            .get(self.selection().page_indices(page_size))
            .into_iter()
            .flatten()
            .copied()
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        self.selection().page_number(page_size)
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        self.selection().page_count(page_size)
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_row_id = None;
            return false;
        }
        if let Some(position) = self
            .selection()
            .select_visible_index(page_size, visible_offset)
        {
            self.selected = position;
            self.sync_selected_row_id();
            true
        } else {
            false
        }
    }

    pub(super) fn selected_position_label(&self) -> usize {
        self.selection().selected_position_label()
    }

    pub(super) fn selected_row(&self) -> Option<&MessageHistoryRow> {
        let row_index = *self.filtered_indices.get(self.selected)?;
        self.rows.get(row_index)
    }

    pub(super) fn selected_row_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    fn select_row_id_or_clamp(&mut self, selected_row_id: Option<i64>) {
        self.restore_selected_row_or_clamp(selected_row_id);
    }

    fn restore_selected_row_or_clamp(&mut self, selected_row_id: Option<i64>) {
        if let Some(selected_row_id) = selected_row_id
            && let Some(position) = self.filtered_indices.iter().position(|row_index| {
                self.rows
                    .get(*row_index)
                    .is_some_and(|row| row.id == selected_row_id)
            })
        {
            self.selected = position;
            self.sync_selected_row_id();
            return;
        }

        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        self.sync_selected_row_id();
    }

    fn sync_selected_row_id(&mut self) {
        self.selected_row_id = self.selected_row().map(|row| row.id);
    }

    /// 复制完整消息正文（列表截断宽度不影响 payload）。
    pub(super) fn copy_payload_full_text(&self) -> Option<String> {
        if let Some(preview) = self.preview.as_ref() {
            return self.rows.get(preview.row_index).map(|row| row.text.clone());
        }
        self.selected_row().map(|row| row.text.clone())
    }
}
