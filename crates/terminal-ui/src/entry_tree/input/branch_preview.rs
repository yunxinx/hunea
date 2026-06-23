use super::*;

impl Model {
    pub(super) fn handle_entry_tree_branch_preview_mouse_down(&mut self, row: u16) -> bool {
        if !self.entry_tree_branch_preview_active() {
            return false;
        }
        if let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        {
            let page_size = self.entry_tree_page_size();
            if let Some(preview) = self
                .entry_tree
                .as_mut()
                .and_then(|state| state.branch_preview.as_mut())
            {
                preview.select_visible_row(page_size, visible_offset);
            }
        }

        true
    }

    pub(crate) fn apply_entry_tree_branch_preview_payload(&mut self, payload: SessionTreePayload) {
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        let Some(preview) = state.branch_preview.as_mut() else {
            return;
        };
        if !preview.is_loading {
            return;
        }
        let current_row_id = payload.current_row_id;
        preview.rows = payload.rows;
        preview.is_loading = false;
        preview.pending_request_id = None;
        preview.error = None;
        preview.message_preview = None;
        if !preview.select_row_by_id(current_row_id.as_deref()) {
            preview.select_latest_row();
        }
    }

    pub(crate) fn show_entry_tree_branch_preview_error(&mut self, message: &str) {
        let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
        else {
            return;
        };
        preview.rows.clear();
        preview.selected = 0;
        preview.is_loading = false;
        preview.pending_request_id = None;
        preview.error = Some(message.to_string());
        preview.message_preview = None;
    }

    pub(crate) fn entry_tree_branch_preview_active(&self) -> bool {
        self.entry_tree
            .as_ref()
            .is_some_and(|state| state.branch_preview.is_some())
    }

    #[cfg(test)]
    pub(crate) fn entry_tree_branch_preview_loading(&self) -> bool {
        self.entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
            .is_some_and(|preview| preview.is_loading)
    }

    pub(crate) fn entry_tree_branch_preview_load_request_matches(
        &self,
        request_id: SessionLoadRequestId,
    ) -> bool {
        self.entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
            .is_some_and(|preview| {
                preview.is_loading && preview.pending_request_id == Some(request_id)
            })
    }

    #[cfg(test)]
    pub(crate) fn entry_tree_branch_preview_pending_request_id_for_test(
        &self,
    ) -> Option<SessionLoadRequestId> {
        self.entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
            .and_then(|preview| preview.pending_request_id)
    }

    #[cfg(test)]
    pub(crate) fn entry_tree_branch_preview_row_ids_for_test(&self) -> Vec<&str> {
        self.entry_tree
            .as_ref()
            .and_then(|state| state.branch_preview.as_ref())
            .map(|preview| {
                preview
                    .rows
                    .iter()
                    .map(|row| row.row_id.as_str())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    pub(super) fn move_entry_tree_branch_preview_selection(
        &mut self,
        direction: ListNavigationDirection,
    ) {
        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
        {
            preview.move_selection(direction);
        }
    }

    pub(super) fn handle_entry_tree_branch_preview_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.entry_tree.as_mut() {
                    state.branch_preview = None;
                }
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_preview_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_entry_tree_branch_preview_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(preview) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_preview.as_mut())
                {
                    preview.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = self.entry_tree_page_size();
                if let Some(preview) = self
                    .entry_tree
                    .as_mut()
                    .and_then(|state| state.branch_preview.as_mut())
                {
                    preview.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_entry_tree_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => OverlayInputResult::Handled,
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }
}
