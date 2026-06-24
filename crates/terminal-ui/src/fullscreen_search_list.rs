use crate::list_selection::{ListNavigationDirection, PagedSelection};

/// 全屏 picker 共用的搜索、过滤、分页与稳定选中状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FullscreenSearchListState<Row, Id> {
    rows: Vec<Row>,
    pub(crate) filtered_indices: Vec<usize>,
    pub(crate) selected: usize,
    selected_id: Option<Id>,
    pub(crate) search_query: String,
    pub(crate) is_searching: bool,
}

impl<Row, Id> Default for FullscreenSearchListState<Row, Id> {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            filtered_indices: Vec::new(),
            selected: 0,
            selected_id: None,
            search_query: String::new(),
            is_searching: false,
        }
    }
}

impl<Row, Id> FullscreenSearchListState<Row, Id>
where
    Id: Clone + Eq,
{
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.filtered_indices.len())
    }

    pub(crate) fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub(crate) fn replace_rows(
        &mut self,
        rows: Vec<Row>,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) {
        self.rows = rows;
        self.apply_filter(matches_query, row_id);
    }

    pub(crate) fn apply_filter(
        &mut self,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) {
        let selected_id = self
            .selected_id
            .clone()
            .or_else(|| self.selected_row().map(&row_id));
        let query = self.search_query.trim();
        self.filtered_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| matches_query(row, query).then_some(index))
            .collect();
        self.restore_selected_id_or_clamp(selected_id, row_id);
    }

    pub(crate) fn select_last_row(&mut self, row_id: impl Fn(&Row) -> Id) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        self.selected = self.filtered_indices.len().saturating_sub(1);
        self.sync_selected_id(row_id);
    }

    pub(crate) fn move_selection(
        &mut self,
        direction: ListNavigationDirection,
        row_id: impl Fn(&Row) -> Id,
    ) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        self.selected = self.selection().move_selection(direction);
        self.sync_selected_id(row_id);
    }

    pub(crate) fn move_page(
        &mut self,
        direction: ListNavigationDirection,
        page_size: usize,
        row_id: impl Fn(&Row) -> Id,
    ) {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        self.selected = self.selection().move_page(direction, page_size);
        self.sync_selected_id(row_id);
    }

    pub(crate) fn push_search_character(
        &mut self,
        character: char,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) {
        self.search_query.push(character);
        self.apply_filter(matches_query, row_id);
    }

    pub(crate) fn backspace_search(
        &mut self,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) {
        if self.search_query.pop().is_some() {
            self.apply_filter(matches_query, row_id);
        }
    }

    pub(crate) fn clear_search(
        &mut self,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) -> bool {
        if self.search_query.is_empty() && !self.is_searching {
            return false;
        }
        let selected_id = self.selected_row().map(&row_id);
        self.search_query.clear();
        self.apply_filter(matches_query, |row| row_id(row));
        self.restore_selected_id_or_clamp(selected_id, row_id);
        true
    }

    pub(crate) fn exit_search(
        &mut self,
        matches_query: impl Fn(&Row, &str) -> bool,
        row_id: impl Fn(&Row) -> Id,
    ) -> bool {
        if !self.is_searching && self.search_query.is_empty() {
            return false;
        }
        let selected_id = self.selected_row().map(&row_id);
        let had_query = !self.search_query.is_empty();
        self.search_query.clear();
        self.is_searching = false;
        if had_query {
            self.apply_filter(matches_query, |row| row_id(row));
            self.restore_selected_id_or_clamp(selected_id, row_id);
        }
        true
    }

    pub(crate) fn page_start(&self, page_size: usize) -> usize {
        self.selection().page_start(page_size)
    }

    pub(crate) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        self.filtered_indices
            .get(self.selection().page_indices(page_size))
            .into_iter()
            .flatten()
            .copied()
    }

    pub(crate) fn page_number(&self, page_size: usize) -> usize {
        self.selection().page_number(page_size)
    }

    pub(crate) fn page_count(&self, page_size: usize) -> usize {
        self.selection().page_count(page_size)
    }

    pub(crate) fn select_visible_row(
        &mut self,
        page_size: usize,
        visible_offset: usize,
        row_id: impl Fn(&Row) -> Id,
    ) -> bool {
        if self.filtered_indices.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return false;
        }
        if let Some(position) = self
            .selection()
            .select_visible_index(page_size, visible_offset)
        {
            self.selected = position;
            self.sync_selected_id(row_id);
            true
        } else {
            false
        }
    }

    pub(crate) fn selected_position_label(&self) -> usize {
        self.selection().selected_position_label()
    }

    pub(crate) fn selected_row(&self) -> Option<&Row> {
        let row_index = *self.filtered_indices.get(self.selected)?;
        self.rows.get(row_index)
    }

    pub(crate) fn selected_row_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    pub(crate) fn sync_selected_id(&mut self, row_id: impl Fn(&Row) -> Id) {
        self.selected_id = self.selected_row().map(row_id);
    }

    fn restore_selected_id_or_clamp(
        &mut self,
        selected_id: Option<Id>,
        row_id: impl Fn(&Row) -> Id,
    ) {
        if let Some(selected_id) = selected_id
            && let Some(position) = self.filtered_indices.iter().position(|row_index| {
                self.rows
                    .get(*row_index)
                    .is_some_and(|row| row_id(row) == selected_id)
            })
        {
            self.selected = position;
            self.sync_selected_id(row_id);
            return;
        }

        self.selected = self
            .selected
            .min(self.filtered_indices.len().saturating_sub(1));
        self.sync_selected_id(row_id);
    }
}

#[cfg(test)]
mod tests;
