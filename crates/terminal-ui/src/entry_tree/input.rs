use super::*;

mod branch_picker;
mod branch_preview;
mod branch_tree;
mod preview;

impl Model {
    pub(crate) fn entry_tree_active(&self) -> bool {
        self.entry_tree.is_some()
    }

    pub(crate) fn entry_tree_loading(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.is_loading)
    }

    pub(crate) fn open_entry_tree_loading(&mut self) {
        self.entry_tree = Some(EntryTreeState {
            is_loading: true,
            ..EntryTreeState::default()
        });
        self.close_composer_attached_ui();
    }

    pub(crate) fn apply_entry_tree_payload(&mut self, payload: SessionTreePayload) {
        let Some(mut state) = self.entry_tree.take() else {
            return;
        };
        let current_row_id = payload.current_row_id;
        state.rows = payload.rows;
        state.is_loading = false;
        state.error = None;
        state.preview = None;
        state.branch_picker = None;
        state.branch_tree = None;
        state.branch_preview = None;
        if !state.select_row_by_id(current_row_id.as_deref()) {
            state.select_latest_row();
        }
        self.entry_tree = Some(state);
    }

    pub(crate) fn show_entry_tree_error(&mut self, message: &str) {
        let Some(mut state) = self.entry_tree.take() else {
            return;
        };
        state.is_loading = false;
        state.error = Some(message.to_string());
        self.entry_tree = Some(state);
    }

    pub(crate) fn move_entry_tree_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        self.move_entry_tree_selection(direction);
    }

    fn move_entry_tree_selection(&mut self, direction: ListNavigationDirection) {
        if self.entry_tree_branch_preview_active() {
            self.move_entry_tree_branch_preview_selection(direction);
            return;
        }
        if self.entry_tree_branch_picker_active() {
            self.move_entry_tree_branch_picker_selection(direction);
            return;
        }
        if self.entry_tree_branch_tree_active() {
            self.move_entry_tree_branch_tree_selection(direction);
            return;
        }
        if let Some(state) = self.entry_tree.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn handle_entry_tree_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if !self.entry_tree_active() {
            return OverlayInputResult::Ignored;
        }

        if self.entry_tree_preview_active() {
            return self.handle_entry_tree_preview_key(key);
        }

        if self.entry_tree_branch_preview_active() {
            return self.handle_entry_tree_branch_preview_key(key);
        }

        if self.entry_tree_branch_picker_active() {
            return self.handle_entry_tree_branch_picker_key(key);
        }

        if self.entry_tree_branch_tree_active() {
            return self.handle_entry_tree_branch_tree_key(key);
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.entry_tree = None;
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(state) = self.entry_tree.as_mut() {
                    state.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_entry_tree_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                self.open_entry_tree_branch_picker();
                OverlayInputResult::Handled
            }
            KeyCode::Char('A') if is_entry_tree_branch_tree_shortcut(key) => {
                OverlayInputResult::Effect(AppEffect::OpenBranchTree)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let selected = self
                    .entry_tree
                    .as_ref()
                    .and_then(EntryTreeState::selected_row);
                if let Some(row) = selected
                    && row.rewind_target_id.is_some()
                {
                    let entry_id = row.row_id.clone();
                    let prefill = row.rewind_prefill.clone();
                    self.entry_tree = None;
                    return OverlayInputResult::Effect(AppEffect::SelectEntryRewind {
                        entry_id,
                        prefill,
                    });
                }
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    pub(crate) fn handle_entry_tree_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.entry_tree_active() {
            return OverlayInputResult::Ignored;
        }

        if button != MouseButton::Left || self.entry_tree_preview_active() {
            return OverlayInputResult::Handled;
        }
        if self.handle_entry_tree_branch_picker_mouse_down(column, row) {
            return OverlayInputResult::Handled;
        }
        if self.handle_entry_tree_branch_preview_mouse_down(row) {
            return OverlayInputResult::Handled;
        }
        if self.handle_entry_tree_branch_tree_mouse_down(row) {
            return OverlayInputResult::Handled;
        }

        let should_close_branch_picker_after_tree_selection =
            self.entry_tree_branch_picker_active() && !self.entry_tree_branch_preview_active();
        let page_size = self.entry_tree_page_size();
        let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        else {
            return OverlayInputResult::Handled;
        };
        if let Some(state) = self.entry_tree.as_mut() {
            let selected_visible_row = state.select_visible_row(page_size, visible_offset);
            if selected_visible_row && should_close_branch_picker_after_tree_selection {
                state.branch_picker = None;
            }
        }
        OverlayInputResult::Handled
    }

    pub(super) fn entry_tree_branch_picker_visible_rows(&self) -> usize {
        usize::from(
            self.branch_picker_list_rows
                .clamp(BRANCH_PICKER_LIST_ROWS_MIN, BRANCH_PICKER_LIST_ROWS_MAX),
        )
    }

    #[cfg(test)]
    pub(crate) fn entry_tree_row_ids_for_test(&self) -> Vec<&str> {
        self.entry_tree
            .as_ref()
            .map(|state| {
                state
                    .rows
                    .iter()
                    .map(|row| row.row_id.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }
}
