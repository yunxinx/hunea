use super::*;

impl Model {
    pub(crate) fn open_entry_tree_branch_tree_loading(&mut self) {
        let mut state = self.entry_tree.take().unwrap_or_default();
        state.preview = None;
        state.branch_picker = None;
        state.branch_preview = None;
        state.branch_tree = Some(EntryTreeBranchTreeState {
            is_loading: true,
            metadata_now_ms: current_unix_timestamp_ms(),
            ..EntryTreeBranchTreeState::default()
        });
        self.entry_tree = Some(state);
    }

    pub(crate) fn apply_entry_tree_branch_tree_payload(
        &mut self,
        payload: SessionBranchTreePayload,
    ) {
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        {
            let Some(branch_tree) = state.branch_tree.as_mut() else {
                return;
            };
            if !branch_tree.is_loading {
                return;
            }
            branch_tree.nodes = branch_tree_display_order_nodes(payload.nodes);
            branch_tree.selected = 0;
            branch_tree.is_loading = false;
            branch_tree.metadata_now_ms = current_unix_timestamp_ms();
            branch_tree.current_branch_row_id = payload.current_branch_row_id;
            branch_tree.total_message_count = payload.total_message_count;
            branch_tree.error = None;
            branch_tree.select_current_or_first();
        }
        state.preview = None;
        state.branch_picker = None;
        state.branch_preview = None;
    }

    pub(crate) fn show_entry_tree_branch_tree_error(&mut self, message: &str) {
        let Some(branch_tree) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_tree.as_mut())
        else {
            return;
        };
        branch_tree.nodes.clear();
        branch_tree.selected = 0;
        branch_tree.is_loading = false;
        branch_tree.error = Some(message.to_string());
    }

    pub(crate) fn entry_tree_branch_tree_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_tree.is_some())
    }

    pub(crate) fn entry_tree_branch_tree_loading(&self) -> bool {
        self.entry_tree
            .as_ref()
            .and_then(|state| state.branch_tree.as_ref())
            .is_some_and(|branch_tree| branch_tree.is_loading)
    }

    pub(super) fn move_entry_tree_branch_tree_selection(
        &mut self,
        direction: ListNavigationDirection,
    ) {
        if let Some(branch_tree) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_tree.as_mut())
        {
            branch_tree.move_selection(direction);
        }
    }

    pub(super) fn handle_entry_tree_branch_tree_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_tree = None;
                    state.branch_preview = None;
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_tree_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_tree_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
                if let Some(branch_tree) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_tree.as_mut())
                {
                    branch_tree.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
                if let Some(branch_tree) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_tree.as_mut())
                {
                    branch_tree.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let Some((branch_row_id, preview_metadata, is_current)) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_tree.as_ref())
                    .and_then(|branch_tree| {
                        let selected_node = branch_tree.selected_node()?;
                        let preview_metadata =
                            EntryTreeBranchPreviewMetadata::from_branch_tree_node(
                                selected_node,
                                branch_tree.metadata_now_ms,
                            );
                        Some((
                            selected_node.branch.branch_row_id.clone(),
                            preview_metadata,
                            selected_node.branch.is_current,
                        ))
                    })
                else {
                    return OverlayInputResult::Handled;
                };
                if is_current {
                    return OverlayInputResult::Handled;
                }
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = Some(EntryTreeBranchPreviewState {
                        metadata: Some(preview_metadata),
                        source: EntryTreeBranchPreviewSource::BranchTree,
                        ..EntryTreeBranchPreviewState::default()
                    });
                }
                OverlayInputResult::Effect(AppEffect::OpenBranchPreview { branch_row_id })
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(selected_node) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_tree.as_ref())
                    .and_then(EntryTreeBranchTreeState::selected_node)
                else {
                    return OverlayInputResult::Handled;
                };
                if selected_node.branch.is_current {
                    return OverlayInputResult::Handled;
                }
                OverlayInputResult::Effect(AppEffect::SwitchBranch {
                    leaf_id: selected_node.branch.subtree_leaf_id.clone(),
                })
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    pub(super) fn handle_entry_tree_branch_tree_mouse_down(&mut self, row: u16) -> bool {
        if !self.entry_tree_branch_tree_active() {
            return false;
        }
        if self.height < ENTRY_TREE_CHROME_HEIGHT {
            return true;
        }

        let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
        let body_height = self.height.saturating_sub(ENTRY_TREE_CHROME_HEIGHT);
        if row > body_top && row < body_top.saturating_add(body_height) {
            let page_size = entry_tree_branch_tree_page_size_for_height(self.height);
            let visible_offset = usize::from(row.saturating_sub(body_top).saturating_sub(1));
            if let Some(branch_tree) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_tree.as_mut())
            {
                branch_tree.select_visible_node(page_size, visible_offset);
            }
        }

        true
    }
}
