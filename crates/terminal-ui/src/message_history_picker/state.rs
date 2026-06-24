use runtime_domain::session::{MessageHistoryRow, SessionLoadRequestId};

use crate::{
    fullscreen_search_list::FullscreenSearchListState, list_selection::ListNavigationDirection,
    text_search::CaseInsensitiveQuery,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageHistoryPickerState {
    pub(super) list: FullscreenSearchListState<MessageHistoryRow, i64>,
    pub(super) opened_at_ms: i64,
    pub(super) pending_request_id: Option<SessionLoadRequestId>,
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
            list: FullscreenSearchListState::default(),
            opened_at_ms: 0,
            pending_request_id: None,
            is_loading: true,
            error: None,
            preview: None,
        }
    }
}

impl MessageHistoryPickerState {
    pub(super) fn replace_rows(&mut self, rows: Vec<MessageHistoryRow>) {
        self.list
            .replace_rows(rows, message_history_row_matches, message_history_row_id);
    }

    pub(super) fn select_latest_row(&mut self) {
        self.list.select_last_row(message_history_row_id);
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.list.move_selection(direction, message_history_row_id);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.list
            .move_page(direction, page_size, message_history_row_id);
    }

    pub(super) fn push_search_character(&mut self, character: char) {
        self.list.push_search_character(
            character,
            message_history_row_matches,
            message_history_row_id,
        );
    }

    pub(super) fn backspace_search(&mut self) {
        self.list
            .backspace_search(message_history_row_matches, message_history_row_id);
    }

    pub(super) fn clear_search(&mut self) -> bool {
        self.list
            .clear_search(message_history_row_matches, message_history_row_id)
    }

    pub(super) fn exit_search(&mut self) -> bool {
        self.list
            .exit_search(message_history_row_matches, message_history_row_id)
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        self.list.page_start(page_size)
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        self.list.page_indices(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        self.list.page_number(page_size)
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        self.list.page_count(page_size)
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        self.list
            .select_visible_row(page_size, visible_offset, message_history_row_id)
    }

    pub(super) fn selected_position_label(&self) -> usize {
        self.list.selected_position_label()
    }

    pub(super) fn filtered_count(&self) -> usize {
        self.list.filtered_count()
    }

    pub(super) fn has_rows(&self) -> bool {
        self.list.has_rows()
    }

    pub(super) fn has_filtered_rows(&self) -> bool {
        self.list.has_filtered_rows()
    }

    #[cfg(test)]
    pub(super) fn selected_visible_position(&self) -> Option<usize> {
        self.list.selected_visible_position()
    }

    pub(super) fn is_selected_visible_position(&self, visible_position: usize) -> bool {
        self.list.is_selected_visible_position(visible_position)
    }

    pub(super) fn selected_row(&self) -> Option<&MessageHistoryRow> {
        self.list.selected_row()
    }

    pub(super) fn row(&self, row_index: usize) -> Option<&MessageHistoryRow> {
        self.list.rows().get(row_index)
    }

    pub(super) fn selected_row_index(&self) -> Option<usize> {
        self.list.selected_row_index()
    }

    #[cfg(test)]
    pub(super) fn filtered_indices_for_test(&self) -> &[usize] {
        self.list.filtered_indices_for_test()
    }

    pub(super) fn is_searching(&self) -> bool {
        self.list.is_searching()
    }

    pub(super) fn search_query(&self) -> &str {
        self.list.search_query()
    }

    pub(super) fn start_search(&mut self) {
        self.list.start_search();
    }

    /// 复制完整消息正文（列表截断宽度不影响 payload）。
    pub(super) fn copy_payload_full_text(&self) -> Option<String> {
        if let Some(preview) = self.preview.as_ref() {
            return self
                .list
                .rows()
                .get(preview.row_index)
                .map(|row| row.text.clone());
        }
        self.selected_row().map(|row| row.text.clone())
    }
}

fn message_history_row_matches(row: &MessageHistoryRow, query: &str) -> bool {
    CaseInsensitiveQuery::new(query).matches(&row.text)
}

fn message_history_row_id(row: &MessageHistoryRow) -> i64 {
    row.id
}
