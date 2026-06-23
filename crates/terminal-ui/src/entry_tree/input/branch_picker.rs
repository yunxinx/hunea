use super::*;

impl Model {
    pub(super) fn handle_entry_tree_branch_picker_mouse_down(
        &mut self,
        column: u16,
        row: u16,
    ) -> bool {
        if self.entry_tree_branch_preview_active() {
            return false;
        }

        let area = Rect::new(0, 0, self.width, self.height);
        if area.is_empty() {
            return false;
        }

        let list_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(popup_area) = self
            .entry_tree
            .as_ref()
            .and_then(|state| state.branch_picker.as_ref().map(|_| state))
            .map(|state| entry_tree_branch_picker_area_for_state(state, area, list_rows))
        else {
            return false;
        };

        if !rect_contains(popup_area, column, row) {
            return false;
        }

        let item_rows = popup_area
            .height
            .saturating_sub(BRANCH_PICKER_CHROME_HEIGHT);
        let item_top = popup_area.y.saturating_add(BRANCH_PICKER_ITEM_TOP_OFFSET);
        if row >= item_top && row < item_top.saturating_add(item_rows) {
            let visible_offset = usize::from(row.saturating_sub(item_top));
            if let Some(picker) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_picker.as_mut())
            {
                picker.select_visible_item(visible_offset, list_rows);
            }
        }

        true
    }

    pub(crate) fn entry_tree_branch_picker_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_picker.is_some())
    }

    pub(super) fn open_entry_tree_branch_picker(&mut self) {
        let visible_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(selected_row) = self
            .entry_tree
            .as_ref()
            .and_then(EntryTreeState::selected_row)
            .cloned()
        else {
            return;
        };
        if selected_row.branch_choices.len() < 2 {
            return;
        }

        let selected = 0;
        if let Some(state) = self.entry_tree.as_mut() {
            let mut branch_picker = EntryTreeBranchPickerState {
                items: selected_row.branch_choices,
                selected,
                scroll: 0,
                metadata_now_ms: current_unix_timestamp_ms(),
                error: None,
            };
            branch_picker.scroll_to_selection(visible_rows);
            state.branch_picker = Some(branch_picker);
            state.preview = None;
            state.branch_preview = None;
        }
    }

    pub(super) fn move_entry_tree_branch_picker_selection(
        &mut self,
        direction: ListNavigationDirection,
    ) {
        let visible_rows = self.entry_tree_branch_picker_visible_rows();
        let Some(picker) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_picker.as_mut())
        else {
            return;
        };
        picker.move_selection(direction, visible_rows);
    }

    pub(super) fn handle_entry_tree_branch_picker_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_picker = None;
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_picker_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_picker_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let Some((branch_row_id, preview_metadata, is_current)) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_picker.as_ref())
                    .and_then(|picker| {
                        let selected_branch = picker.selected_item()?;
                        let preview_metadata = EntryTreeBranchPreviewMetadata::from_branch_choice(
                            selected_branch,
                            picker.metadata_now_ms,
                        );
                        Some((
                            selected_branch.branch.branch_row_id.clone(),
                            preview_metadata,
                            selected_branch.branch.is_current,
                        ))
                    })
                else {
                    return OverlayInputResult::Handled;
                };
                if is_current {
                    return OverlayInputResult::Handled;
                }
                let request_id = self.next_session_load_request_id();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = Some(EntryTreeBranchPreviewState {
                        pending_request_id: Some(request_id),
                        metadata: Some(preview_metadata),
                        source: EntryTreeBranchPreviewSource::BranchPicker,
                        ..EntryTreeBranchPreviewState::default()
                    });
                    if let Some(picker) = state.branch_picker.as_mut() {
                        picker.error = None;
                    }
                }
                OverlayInputResult::Effect(AppEffect::OpenBranchPreview {
                    request_id,
                    branch_row_id,
                })
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let Some(leaf_id) = self
                    .entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_picker.as_ref())
                    .and_then(EntryTreeBranchPickerState::selected_item)
                    .map(|branch| branch.branch.subtree_leaf_id.clone())
                else {
                    return OverlayInputResult::Handled;
                };
                OverlayInputResult::Effect(AppEffect::SwitchBranch { leaf_id })
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    pub(crate) fn show_entry_tree_branch_picker_error(&mut self, message: &str) {
        let Some(picker) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_picker.as_mut())
        else {
            self.show_toast(ToastSeverity::Error, message);
            return;
        };
        picker.error = Some(message.to_string());
    }
}
