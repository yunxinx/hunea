use super::*;

impl Model {
    pub(crate) fn move_entry_tree_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.overlay.scroll_offset = entry_tree_preview_page_offset(
                &mut preview.transcript,
                content_height,
                preview.overlay.scroll_offset,
                direction,
            );
            preview.is_following_bottom = false;
            return;
        }

        if let Some(preview) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
            .and_then(|preview| preview.message_preview.as_mut())
        {
            preview.overlay.scroll_offset = entry_tree_preview_page_offset(
                &mut preview.transcript,
                content_height,
                preview.overlay.scroll_offset,
                direction,
            );
            preview.is_following_bottom = false;
        }
    }

    pub(crate) fn entry_tree_preview_active(&self) -> bool {
        self.entry_tree.as_ref().is_some_and(|state| {
            state.preview.is_some()
                || state
                    .branch_preview
                    .as_ref()
                    .is_some_and(|preview| preview.message_preview.is_some())
        })
    }

    pub(super) fn open_entry_tree_preview(&mut self) {
        let from_branch_preview = self.entry_tree_branch_preview_active();
        let mut transcript = {
            let selected_row = if from_branch_preview {
                self.entry_tree
                    .as_ref()
                    .and_then(|state| state.branch_preview.as_ref())
                    .and_then(EntryTreeBranchPreviewState::selected_row)
            } else {
                self.entry_tree
                    .as_ref()
                    .and_then(EntryTreeState::selected_row)
            };
            let Some(row) = selected_row else {
                return;
            };
            self.transcript_from_session_tree_preview_replay_with_tool_activity_render_mode(
                SessionTreePreviewReplay::from_session_tree_row(row),
                ToolActivityRenderMode::DebugDetailed,
            )
        };

        transcript.set_reasoning_render_mode(ReasoningRenderMode::Detailed);
        let content_height = self.transcript_overlay_content_height();
        let mut preview = EntryTreePreviewState::following_bottom(transcript);
        preview.sync_follow_bottom(content_height);

        if let Some(preview_state) = self
            .entry_tree
            .as_mut()
            .and_then(|state| state.branch_preview.as_mut())
            .filter(|_| from_branch_preview)
        {
            preview_state.message_preview = Some(preview);
        } else if let Some(state) = self.entry_tree.as_mut() {
            state.preview = Some(preview);
        }
    }

    pub(crate) fn sync_entry_tree_preview_follow_bottom(&mut self) {
        let content_height = self.transcript_overlay_content_height();
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        if let Some(preview) = state
            .preview
            .as_mut()
            .filter(|preview| preview.is_following_bottom)
        {
            preview.sync_follow_bottom(content_height);
        }
        if let Some(preview) = state
            .branch_preview
            .as_mut()
            .and_then(|preview| preview.message_preview.as_mut())
            .filter(|preview| preview.is_following_bottom)
        {
            preview.sync_follow_bottom(content_height);
        }
    }

    pub(crate) fn sync_entry_tree_preview_width(&mut self, width: u16) {
        let content_height = self.transcript_overlay_content_height();
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        if let Some(preview) = state.preview.as_mut() {
            preview.set_width(width, content_height);
        }
        if let Some(preview) = state
            .branch_preview
            .as_mut()
            .and_then(|preview| preview.message_preview.as_mut())
        {
            preview.set_width(width, content_height);
        }
    }

    pub(crate) fn sync_entry_tree_preview_palette(&mut self, palette: TerminalPalette) {
        let content_height = self.transcript_overlay_content_height();
        let Some(state) = self.entry_tree.as_mut() else {
            return;
        };
        if let Some(preview) = state.preview.as_mut() {
            preview.set_palette(palette, content_height);
        }
        if let Some(preview) = state
            .branch_preview
            .as_mut()
            .and_then(|preview| preview.message_preview.as_mut())
        {
            preview.set_palette(palette, content_height);
        }
    }

    fn close_entry_tree_preview(&mut self) {
        if let Some(state) = self.entry_tree.as_mut() {
            if state.preview.is_some() {
                state.preview = None;
                return;
            }
            if let Some(preview) = state.branch_preview.as_mut() {
                preview.message_preview = None;
            }
        }
    }

    pub(super) fn handle_entry_tree_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_entry_tree_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_entry_tree_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }
}
