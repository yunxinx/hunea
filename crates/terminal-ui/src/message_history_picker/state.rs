use runtime_domain::session::MessageHistoryRow;

use crate::{
    list_selection::{ListNavigationDirection, PagedSelection},
    text_search::contains_case_insensitive,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageHistoryPickerState {
    pub(super) rows: Vec<MessageHistoryRow>,
    pub(super) filtered_indices: Vec<usize>,
    pub(super) selected: usize,
    pub(super) opened_at_ms: i64,
    pub(super) search_query: String,
    pub(super) is_searching: bool,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
    pub(super) preview: Option<MessageHistoryPickerPreviewState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MessageHistoryPickerPreviewState {
    pub(super) row_index: usize,
    pub(super) transcript_preview: crate::transcript_preview::TranscriptPreviewState,
}

impl Default for MessageHistoryPickerState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            opened_at_ms: 0,
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
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (query.is_empty() || message_history_row_matches(&row.text, query)).then_some(index)
            })
            .collect();
        self.restore_selected_row_or_clamp();
    }

    pub(super) fn select_latest_row(&mut self) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self.filtered_indices.len().saturating_sub(1);
    }

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self.selection().move_selection(direction);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = self.selection().move_page(direction, page_size);
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
        self.select_filtered_row_index(selected_row_index);
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
            self.select_filtered_row_index(selected_row_index);
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

    pub(super) fn select_filtered_row_index(&mut self, row_index: Option<usize>) {
        if let Some(row_index) = row_index
            && let Some(position) = self
                .filtered_indices
                .iter()
                .position(|filtered_index| *filtered_index == row_index)
        {
            self.selected = position;
            return;
        }

        self.restore_selected_row_or_clamp();
    }

    fn restore_selected_row_or_clamp(&mut self) {
        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
    }

    /// 复制完整消息正文（列表截断宽度不影响 payload）。
    pub(super) fn copy_payload_full_text(&self) -> Option<String> {
        if let Some(preview) = self.preview.as_ref() {
            return self.rows.get(preview.row_index).map(|row| row.text.clone());
        }
        self.selected_row().map(|row| row.text.clone())
    }
}

fn message_history_row_matches(text: &str, query: &str) -> bool {
    contains_case_insensitive(text, query)
}
