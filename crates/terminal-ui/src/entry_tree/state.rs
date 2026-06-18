use runtime_domain::session::{SessionBranchTreeNode, SessionTreeBranchChoice, SessionTreeRow};

use crate::{
    list_selection::{ListNavigationDirection, PagedSelection, VisibleWindowSelection},
    transcript_preview::TranscriptPreviewState,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub(super) type EntryTreePreviewState = TranscriptPreviewState;

impl EntryTreeState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.rows.len())
    }

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

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.selected = self.selection().move_selection(direction);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.selected = self.selection().move_page(direction, page_size);
    }

    pub(super) fn selected_row(&self) -> Option<&SessionTreeRow> {
        self.rows.get(self.selected)
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
}

impl EntryTreeBranchPickerState {
    fn selection(&self) -> VisibleWindowSelection {
        VisibleWindowSelection::new(self.selected, self.items.len())
    }

    pub(super) fn selected_item(&self) -> Option<&SessionTreeBranchChoice> {
        self.items.get(self.selected)
    }

    pub(super) fn move_selection(
        &mut self,
        direction: ListNavigationDirection,
        visible_rows: usize,
    ) {
        self.selected = self.selection().move_selection(direction);
        if self.items.is_empty() {
            self.scroll = 0;
            return;
        }
        self.scroll = self
            .selection()
            .scroll_start_for_selection(self.scroll, visible_rows);
    }

    pub(super) fn scroll_to_selection(&mut self, visible_rows: usize) {
        self.scroll = self
            .selection()
            .scroll_start_for_selection(self.scroll, visible_rows);
    }

    pub(super) fn select_visible_item(
        &mut self,
        visible_offset: usize,
        visible_rows: usize,
    ) -> bool {
        let Some(selected) = self
            .selection()
            .select_visible_index(self.scroll, visible_offset)
        else {
            return false;
        };
        self.selected = selected;
        self.scroll_to_selection(visible_rows);
        true
    }
}

impl EntryTreeBranchTreeState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.nodes.len())
    }

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

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.selected = self.selection().move_selection(direction);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.selected = self.selection().move_page(direction, page_size);
    }

    pub(super) fn select_visible_node(&mut self, page_size: usize, visible_offset: usize) -> bool {
        if let Some(node_index) = self
            .selection()
            .select_visible_index(page_size, visible_offset)
        {
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
}

impl EntryTreeBranchPreviewState {
    fn selection(&self) -> PagedSelection {
        PagedSelection::new(self.selected, self.rows.len())
    }

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

    pub(super) fn move_selection(&mut self, direction: ListNavigationDirection) {
        self.selected = self.selection().move_selection(direction);
    }

    pub(super) fn move_page(&mut self, direction: ListNavigationDirection, page_size: usize) {
        self.selected = self.selection().move_page(direction, page_size);
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

    pub(super) fn selected_row(&self) -> Option<&SessionTreeRow> {
        self.rows.get(self.selected)
    }

    pub(super) fn selected_position_label(&self) -> usize {
        self.selection().selected_position_label()
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

#[cfg(test)]
mod tests {
    use crate::theme::default_palette;
    use crate::transcript::Transcript;
    use crate::transcript_overlay::TranscriptOverlayState;
    use crate::transcript_preview::TranscriptPreviewState;

    #[test]
    fn transcript_preview_equality_includes_bottom_follow_state() {
        let following = TranscriptPreviewState {
            transcript: Transcript::new(default_palette()),
            overlay: TranscriptOverlayState::new(),
            is_following_bottom: true,
        };
        let manually_scrolled = TranscriptPreviewState {
            is_following_bottom: false,
            ..following.clone()
        };

        assert_ne!(
            following, manually_scrolled,
            "preview equality must include runtime scroll-follow behavior"
        );
    }
}
