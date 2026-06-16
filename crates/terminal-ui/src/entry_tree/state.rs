use runtime_domain::session::{SessionBranchTreeNode, SessionTreeBranchChoice, SessionTreeRow};

use crate::{transcript::Transcript, transcript_overlay::TranscriptOverlayState};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EntryTreeState {
    pub(super) rows: Vec<SessionTreeRow>,
    pub(super) selected: usize,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
    pub(super) preview: Option<EntryTreePreviewState>,
    pub(super) branch_picker: Option<EntryTreeBranchPickerState>,
    pub(super) branch_tree: Option<EntryTreeBranchTreeState>,
    pub(super) branch_preview: Option<EntryTreeBranchPreviewState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct EntryTreeBranchPickerState {
    pub(super) items: Vec<SessionTreeBranchChoice>,
    pub(super) selected: usize,
    pub(super) scroll: usize,
    pub(super) metadata_now_ms: i64,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct EntryTreeBranchTreeState {
    pub(super) nodes: Vec<SessionBranchTreeNode>,
    pub(super) selected: usize,
    pub(super) is_loading: bool,
    pub(super) metadata_now_ms: i64,
    pub(super) current_branch_row_id: Option<String>,
    pub(super) total_message_count: usize,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct EntryTreeBranchPreviewState {
    pub(super) rows: Vec<SessionTreeRow>,
    pub(super) selected: usize,
    pub(super) is_loading: bool,
    pub(super) error: Option<String>,
    pub(super) message_preview: Option<EntryTreePreviewState>,
    pub(super) metadata: Option<EntryTreeBranchPreviewMetadata>,
    pub(super) source: EntryTreeBranchPreviewSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntryTreeBranchPreviewMetadata {
    pub(super) branch_created_at_ms: i64,
    pub(super) latest_updated_at_ms: i64,
    pub(super) metadata_now_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum EntryTreeBranchPreviewSource {
    #[default]
    BranchPicker,
    BranchTree,
}

impl PartialEq for EntryTreeBranchPreviewState {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows
            && self.selected == other.selected
            && self.is_loading == other.is_loading
            && self.error == other.error
            && self.message_preview == other.message_preview
            && self.metadata == other.metadata
            && self.source == other.source
    }
}

impl Eq for EntryTreeBranchPreviewState {}

impl Default for EntryTreeBranchPreviewState {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            is_loading: true,
            error: None,
            message_preview: None,
            metadata: None,
            source: EntryTreeBranchPreviewSource::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct EntryTreePreviewState {
    pub(super) transcript: Transcript,
    pub(super) overlay: TranscriptOverlayState,
    pub(super) is_following_bottom: bool,
}

impl PartialEq for EntryTreePreviewState {
    fn eq(&self, other: &Self) -> bool {
        self.transcript == other.transcript
            && self.overlay == other.overlay
            && self.is_following_bottom == other.is_following_bottom
    }
}

impl Eq for EntryTreePreviewState {}

impl EntryTreeState {
    pub(super) fn select_latest_row(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    pub(super) fn select_row_by_id(&mut self, row_id: Option<&str>) -> bool {
        let Some(row_id) = row_id else {
            return false;
        };
        let Some(index) = self.rows.iter().position(|row| row.row_id == row_id) else {
            return false;
        };
        self.selected = index;
        true
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
    }

    pub(super) fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.rows.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.rows.len().saturating_sub(1));
    }

    pub(super) fn selected_row(&self) -> Option<&SessionTreeRow> {
        self.rows.get(self.selected)
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        let row_index = self.page_start(page_size).saturating_add(visible_offset);
        if row_index < self.rows.len() {
            self.selected = row_index;
            true
        } else {
            false
        }
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        let page_size = page_size.max(1);
        (self.page_start(page_size)..self.rows.len()).take(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        if self.rows.is_empty() {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        if self.rows.is_empty() {
            return 1;
        }
        self.rows.len().saturating_sub(1) / page_size.max(1) + 1
    }

    pub(super) fn selected_position_label(&self) -> usize {
        if self.rows.is_empty() {
            0
        } else {
            self.selected + 1
        }
    }
}

impl EntryTreeBranchPickerState {
    pub(super) fn selected_item(&self) -> Option<&SessionTreeBranchChoice> {
        self.items.get(self.selected)
    }

    pub(super) fn move_selection(&mut self, direction: isize, visible_rows: usize) {
        if self.items.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        let last = self.items.len() - 1;
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
        self.scroll =
            clamp_branch_picker_scroll(self.scroll, self.selected, self.items.len(), visible_rows);
    }
}

impl EntryTreeBranchTreeState {
    pub(super) fn selected_node(&self) -> Option<&SessionBranchTreeNode> {
        self.nodes.get(self.selected)
    }

    pub(super) fn select_current_or_first(&mut self) {
        if self.nodes.is_empty() {
            self.selected = 0;
            return;
        }
        if let Some(current_branch_row_id) = self.current_branch_row_id.as_deref()
            && let Some(index) = self
                .nodes
                .iter()
                .position(|node| node.branch.branch_row_id == current_branch_row_id)
        {
            self.selected = index;
            return;
        }
        if let Some(index) = self.nodes.iter().position(|node| node.branch.is_current) {
            self.selected = index;
            return;
        }
        self.selected = 0;
    }

    pub(super) fn move_selection(&mut self, direction: isize, visible_rows: usize) {
        if self.nodes.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.nodes.len() - 1;
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
        let page_size = visible_rows.max(1);
        let page_start = self.page_start(page_size);
        if self.selected < page_start || self.selected >= page_start.saturating_add(page_size) {
            let next_page_start = self.selected / page_size * page_size;
            self.selected = self.selected.max(next_page_start);
        }
    }

    pub(super) fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.nodes.is_empty() {
            self.selected = 0;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.nodes.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.nodes.len().saturating_sub(1));
    }

    pub(super) fn select_visible_node(&mut self, page_size: usize, visible_offset: usize) -> bool {
        let node_index = self.page_start(page_size).saturating_add(visible_offset);
        if node_index < self.nodes.len() {
            self.selected = node_index;
            true
        } else {
            false
        }
    }

    pub(super) fn selected_visible_row(&self, page_size: usize) -> Option<usize> {
        if self.nodes.is_empty() {
            return None;
        }
        self.selected.checked_sub(self.page_start(page_size))
    }

    pub(super) fn page_start(&self, page_size: usize) -> usize {
        let page_size = page_size.max(1);
        self.selected / page_size * page_size
    }

    pub(super) fn page_indices(&self, page_size: usize) -> impl Iterator<Item = usize> + '_ {
        let page_size = page_size.max(1);
        (self.page_start(page_size)..self.nodes.len()).take(page_size)
    }

    pub(super) fn page_number(&self, page_size: usize) -> usize {
        if self.nodes.is_empty() {
            return 1;
        }
        self.selected / page_size.max(1) + 1
    }

    pub(super) fn page_count(&self, page_size: usize) -> usize {
        if self.nodes.is_empty() {
            return 1;
        }
        self.nodes.len().saturating_sub(1) / page_size.max(1) + 1
    }

    pub(super) fn selected_position_label(&self) -> usize {
        if self.nodes.is_empty() {
            0
        } else {
            self.selected + 1
        }
    }
}

impl EntryTreeBranchPreviewState {
    pub(super) fn select_latest_row(&mut self) {
        self.selected = self.rows.len().saturating_sub(1);
    }

    pub(super) fn select_row_by_id(&mut self, row_id: Option<&str>) -> bool {
        let Some(row_id) = row_id else {
            return false;
        };
        let Some(index) = self.rows.iter().position(|row| row.row_id == row_id) else {
            return false;
        };
        self.selected = index;
        true
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let last = self.rows.len().saturating_sub(1);
        self.selected = if direction.is_negative() {
            self.selected.saturating_sub(direction.unsigned_abs())
        } else {
            self.selected.saturating_add(direction as usize).min(last)
        };
    }

    pub(super) fn move_page(&mut self, direction: isize, page_size: usize) {
        if self.rows.is_empty() {
            self.selected = 0;
            return;
        }
        let page_size = page_size.max(1);
        let current_page = self.selected / page_size;
        let last_page = self.rows.len().saturating_sub(1) / page_size;
        let next_page = if direction.is_negative() {
            current_page.saturating_sub(direction.unsigned_abs())
        } else {
            current_page
                .saturating_add(direction as usize)
                .min(last_page)
        };
        self.selected = (next_page * page_size).min(self.rows.len().saturating_sub(1));
    }

    pub(super) fn select_visible_row(&mut self, page_size: usize, visible_offset: usize) -> bool {
        let page_size = page_size.max(1);
        let page_start = self.selected / page_size * page_size;
        let row_index = page_start.saturating_add(visible_offset);
        if row_index < self.rows.len() {
            self.selected = row_index;
            true
        } else {
            false
        }
    }

    pub(super) fn selected_row(&self) -> Option<&SessionTreeRow> {
        self.rows.get(self.selected)
    }

    pub(super) fn selected_position_label(&self) -> usize {
        if self.rows.is_empty() {
            0
        } else {
            self.selected + 1
        }
    }
}

impl EntryTreeBranchPreviewMetadata {
    pub(super) fn from_branch_choice(item: &SessionTreeBranchChoice, metadata_now_ms: i64) -> Self {
        Self {
            branch_created_at_ms: item.branch.branch_created_at_ms,
            latest_updated_at_ms: item.branch.latest_updated_at_ms,
            metadata_now_ms,
        }
    }

    pub(super) fn from_branch_tree_node(
        node: &SessionBranchTreeNode,
        metadata_now_ms: i64,
    ) -> Self {
        Self {
            branch_created_at_ms: node.branch.branch_created_at_ms,
            latest_updated_at_ms: node.branch.latest_updated_at_ms,
            metadata_now_ms,
        }
    }
}

pub(super) fn clamp_branch_picker_scroll(
    scroll: usize,
    selected: usize,
    item_count: usize,
    visible_rows: usize,
) -> usize {
    if item_count == 0 {
        return 0;
    }
    let visible_rows = visible_rows.max(1);
    let max_scroll = item_count.saturating_sub(visible_rows);
    let mut scroll = scroll.min(max_scroll);
    if selected < scroll {
        scroll = selected;
    }
    if selected >= scroll.saturating_add(visible_rows) {
        scroll = selected + 1 - visible_rows;
    }
    scroll.min(max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;

    #[test]
    fn entry_tree_preview_equality_includes_bottom_follow_state() {
        let following = EntryTreePreviewState {
            transcript: Transcript::new(default_palette()),
            overlay: TranscriptOverlayState::new(),
            is_following_bottom: true,
        };
        let manually_scrolled = EntryTreePreviewState {
            is_following_bottom: false,
            ..following.clone()
        };

        assert_ne!(
            following, manually_scrolled,
            "preview equality must include runtime scroll-follow behavior"
        );
    }
}
