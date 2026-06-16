use runtime_domain::session::SessionPickerRow;

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
    pub(super) fn apply_filter(&mut self) {
        let query = self.search_query.trim();
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (query.is_empty() || session_picker_row_matches(row, query)).then_some(index)
            })
            .collect();
        self.restore_selected_session_or_clamp();
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        let last = self.filtered_indices.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
        self.sync_selected_session_id();
    }

    pub(super) fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_session_id = None;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.filtered_indices.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.filtered_indices.len().saturating_sub(1));
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
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        let page_size = page_size.max(1);
        self.filtered_indices
            .iter()
            .skip(self.page_start(page_size))
            .take(page_size)
            .copied()
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        if self.filtered_indices.is_empty() {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.filtered_indices.len().saturating_sub(1) / page_size + 1
    }

    pub(super) fn selected_position_label(&self) -> usize {
        if self.filtered_indices.is_empty() {
            0
        } else {
            self.selected + 1
        }
    }
}

fn session_picker_row_matches(row: &SessionPickerRow, query: &str) -> bool {
    contains_case_insensitive(&row.title, query)
        || contains_case_insensitive(&row.first_user_message, query)
        || contains_case_insensitive(&row.last_assistant_message, query)
        || contains_case_insensitive(&row.work_dir, query)
        || row
            .model
            .as_deref()
            .is_some_and(|model| contains_case_insensitive(model, query))
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let needle_bytes = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes));
    }

    haystack.to_lowercase().contains(&needle.to_lowercase())
}
