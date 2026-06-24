use runtime_domain::session::SessionPickerRow;

use crate::{
    fullscreen_search_list::FullscreenSearchListState, list_selection::ListNavigationDirection,
    text_search::CaseInsensitiveQuery,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionPickerState {
    pub(super) list: FullscreenSearchListState<SessionPickerRow, String>,
    pub(super) opened_at_ms: i64,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
}

impl SessionPickerState {
    pub(super) fn replace_rows(&mut self, rows: Vec<SessionPickerRow>) {
        self.list
            .replace_rows(rows, session_picker_row_matches, session_picker_row_id);
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.list.move_selection(direction, session_picker_row_id);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.list
            .move_page(direction, page_size, session_picker_row_id);
    }

    pub(super) fn push_search_character(&mut self, character: char) {
        self.list.push_search_character(
            character,
            session_picker_row_matches,
            session_picker_row_id,
        );
    }

    pub(super) fn backspace_search(&mut self) {
        self.list
            .backspace_search(session_picker_row_matches, session_picker_row_id);
    }

    pub(super) fn clear_search(&mut self) -> bool {
        self.list
            .clear_search(session_picker_row_matches, session_picker_row_id)
    }

    pub(super) fn exit_search(&mut self) -> bool {
        self.list
            .exit_search(session_picker_row_matches, session_picker_row_id)
    }

    pub(super) fn selected_row(&self) -> Option<&SessionPickerRow> {
        self.list.selected_row()
    }

    pub(super) fn row(&self, row_index: usize) -> Option<&SessionPickerRow> {
        self.list.rows().get(row_index)
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

    pub(super) fn selected_position_label(&self) -> usize {
        self.list.selected_position_label()
    }

    pub(super) fn filtered_count(&self) -> usize {
        self.list.filtered_count()
    }

    #[cfg(test)]
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

    pub(super) fn is_searching(&self) -> bool {
        self.list.is_searching()
    }

    pub(super) fn search_query(&self) -> &str {
        self.list.search_query()
    }

    pub(super) fn start_search(&mut self) {
        self.list.start_search();
    }
}

fn session_picker_row_matches(row: &SessionPickerRow, query: &CaseInsensitiveQuery<'_>) -> bool {
    query.matches(&row.title)
        || query.matches(&row.first_user_message)
        || query.matches(&row.last_assistant_message)
        || query.matches(&row.work_dir)
        || row
            .model
            .as_deref()
            .is_some_and(|model| query.matches(model))
}

fn session_picker_row_id(row: &SessionPickerRow) -> String {
    row.session_id.clone()
}
