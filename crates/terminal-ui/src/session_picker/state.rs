use runtime_domain::session::SessionPickerRow;

use crate::{
    list_selection::{ListNavigationDirection, PagedSelection},
    text_search::CaseInsensitiveQuery,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SessionPickerState {
    pub(super) rows: Vec<SessionPickerRow>,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) selected: usize,
    pub(super) selected_session_id: Option<String>,
    pub(super) opened_at_ms: i64,
    pub(super) search_query: String,
    pub(super) is_searching: bool,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
}

impl SessionPickerState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.filtered_indices.len())
    }

    pub(super) fn apply_filter(&mut self) {
        let query = self.search_query.trim();
        let query = CaseInsensitiveQuery::new(query);
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| session_picker_row_matches(row, &query).then_some(index))
            .collect();
        self.restore_selected_session_or_clamp();
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        self.selected = self.selection().move_selection(direction);
        self.sync_selected_session_id();
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        self.selected = self.selection().move_page(direction, page_size);
        self.sync_selected_session_id();
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
        let selected_row_index = self.selected_row_index();
        self.search_query.clear();
        self.apply_filter();
        self.select_filtered_row_index_or_session(selected_row_index);
        true
    }

    pub(super) fn exit_search(&mut self) -> bool {
        if !self.is_searching && self.search_query.is_empty() {
            return false;
        }
        let selected_row_index = self.selected_row_index();
        let had_query = !self.search_query.is_empty();
        self.search_query.clear();
        self.is_searching = false;
        if had_query {
            self.apply_filter();
            self.select_filtered_row_index_or_session(selected_row_index);
        }
        true
    }

    pub(super) fn selected_row(&self) -> Option<&SessionPickerRow> {
        let row_index = *self.filtered_indices.get(self.selected)?;
        self.rows.get(row_index)
    }

    pub(super) fn selected_row_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    pub(super) fn select_filtered_row_index_or_session(&mut self, row_index: Option<usize>) {
        if let Some(row_index) = row_index
            && let Some(position) = self
                .filtered_indices
                .iter()
                .position(|filtered_index| *filtered_index == row_index)
        {
            self.selected = position;
            self.sync_selected_session_id();
            return;
        }

        self.restore_selected_session_or_clamp();
    }

    pub(super) fn restore_selected_session_or_clamp(&mut self) {
        if let Some(selected_session_id) = self.selected_session_id.as_deref()
            && let Some(position) = self.filtered_indices.iter().position(|row_index| {
                self.rows
                    .get(*row_index)
                    .is_some_and(|row| row.session_id == selected_session_id)
            })
        {
            self.selected = position;
            return;
        }

        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        self.sync_selected_session_id();
    }

    pub(super) fn sync_selected_session_id(&mut self) {
        self.selected_session_id = self.selected_row().map(|row| row.session_id.clone());
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

    pub(super) fn selected_position_label(&self) -> usize {
        self.selection().selected_position_label()
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
