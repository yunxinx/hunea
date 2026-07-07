use super::*;
use crate::plain_text_preview::MessagePreviewMode;

#[derive(Debug, Clone)]
pub(crate) struct CopyPickerState {
    pub(super) rows: Vec<CopyPickerRow>,
    pub(super) selected: usize,
    pub(super) selected_row_indices: BTreeSet<usize>,
    pub(super) is_loading: bool,
    pub(super) pending_request_id: Option<SessionLoadRequestId>,
    pub(super) error: Option<String>,
    pub(super) preview: Option<CopyPickerPreviewState>,
}

#[derive(Debug, Clone)]
pub(super) struct CopyPickerRow {
    pub(super) row_id: String,
    pub(super) kind: CopyableSessionTreeRowKind,
    pub(super) summary: String,
    pub(super) raw_text: String,
    pub(super) replay_items: Vec<TranscriptReplayItem>,
}

#[derive(Debug, Clone)]
pub(super) struct CopyPickerPreviewState {
    pub(super) row_index: usize,
    pub(super) mode: MessagePreviewMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CopyPickerTextFormat {
    Raw,
    Display,
}

impl Default for CopyPickerState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            selected_row_indices: BTreeSet::new(),
            is_loading: true,
            pending_request_id: None,
            error: None,
            preview: None,
        }
    }
}

impl CopyPickerState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.rows.len())
    }

    pub(super) fn selected_row(&self) -> Option<&CopyPickerRow> {
        self.rows.get(self.selected)
    }

    pub(super) fn select_row_by_id(&mut self, row_id: Option<&str>) -> bool {
        let Some(index) = row_index_by_id(&self.rows, row_id, CopyPickerRow::row_id) else {
            return false;
        };
        self.selected = index;
        true
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

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        if let Some(row_index) = self
            .selection()
            .select_visible_index(page_size, visible_offset)
        {
            self.selected = row_index;
            true
        } else {
            false
        }
    }

    pub(super) fn selected_count(&self) -> usize {
        self.selected_row_indices.len()
    }

    pub(super) fn is_row_selected(&self, row_index: usize) -> bool {
        self.selected_row_indices.contains(&row_index)
    }

    pub(super) fn remap_selected_rows_from_previous_rows(
        &mut self,
        previous_rows: &[CopyPickerRow],
    ) {
        let previous_selected_ids = self
            .selected_row_indices
            .iter()
            .filter_map(|row_index| previous_rows.get(*row_index))
            .map(|row| row.row_id.as_str())
            .collect::<BTreeSet<_>>();
        self.selected_row_indices = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                previous_selected_ids
                    .contains(row.row_id.as_str())
                    .then_some(index)
            })
            .collect();
    }

    pub(super) fn toggle_selected_row(&mut self) {
        if self.selected_row().is_none() {
            return;
        };
        if !self.selected_row_indices.remove(&self.selected) {
            self.selected_row_indices.insert(self.selected);
        }
    }

    pub(super) fn select_all_or_invert(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.selected_row_indices.is_empty() {
            self.selected_row_indices = (0..self.rows.len()).collect();
            return;
        }

        self.selected_row_indices = (0..self.rows.len())
            .filter(|row_index| !self.selected_row_indices.contains(row_index))
            .collect();
    }

    pub(super) fn copy_payload(&self, format: CopyPickerTextFormat) -> Option<String> {
        if let Some(preview) = self.preview.as_ref() {
            return self
                .rows
                .get(preview.row_index)
                .map(|row| row.copy_text_for_format(format));
        }

        let mut payload = String::new();
        let mut has_rows = false;
        if self.selected_row_indices.is_empty() {
            if let Some(row) = self.selected_row() {
                append_copy_payload_row(&mut payload, &mut has_rows, row, format);
            }
        } else {
            for row_index in &self.selected_row_indices {
                if let Some(row) = self.rows.get(*row_index) {
                    append_copy_payload_row(&mut payload, &mut has_rows, row, format);
                }
            }
        }

        has_rows.then_some(payload)
    }
}

fn append_copy_payload_row(
    payload: &mut String,
    has_rows: &mut bool,
    row: &CopyPickerRow,
    format: CopyPickerTextFormat,
) {
    if *has_rows {
        payload.push_str(COPY_PICKER_JOIN_SEPARATOR);
    } else {
        *has_rows = true;
    }
    row.append_text_for_format(format, payload);
}

impl CopyPickerRow {
    pub(super) fn row_id(&self) -> &str {
        &self.row_id
    }

    pub(super) fn from_session_tree_row(row: SessionTreeRow) -> Option<Self> {
        if !session_tree_row_kind_is_copyable(row.kind) {
            return None;
        }
        let kind = CopyableSessionTreeRowKind::from_session_tree_kind(row.kind)?;
        Some(Self {
            row_id: row.row_id,
            kind,
            summary: row.summary,
            raw_text: row.preview_content,
            replay_items: row.preview_replay_items,
        })
    }

    pub(super) fn copy_text_for_format(&self, format: CopyPickerTextFormat) -> String {
        let mut text = String::new();
        self.append_text_for_format(format, &mut text);
        text
    }

    pub(super) fn preview_replay(&self) -> SessionTreePreviewReplay<'_> {
        SessionTreePreviewReplay::from_copyable_parts(self.kind, &self.replay_items, &self.raw_text)
    }

    pub(super) fn preview_body_text(&self) -> String {
        crate::plain_text_preview::preview_body_text(&self.raw_text, &self.replay_items)
    }

    fn append_text_for_format(&self, format: CopyPickerTextFormat, text: &mut String) {
        match format {
            CopyPickerTextFormat::Raw => text.push_str(&self.raw_text),
            CopyPickerTextFormat::Display => self.append_display_text(text),
        }
    }

    fn append_display_text(&self, text: &mut String) {
        let mut has_replay_text = false;
        for replay_text in self
            .replay_items
            .iter()
            .map(TranscriptReplayItem::content_text)
            .filter(|text| !text.trim().is_empty())
        {
            if has_replay_text {
                text.push_str("\n\n");
            } else {
                has_replay_text = true;
            }
            text.push_str(replay_text);
        }

        if !has_replay_text {
            text.push_str(&self.raw_text);
        }
    }
}
