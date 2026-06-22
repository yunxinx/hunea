use super::*;

impl Model {
    pub(crate) fn copy_picker_active(&self) -> bool {
        self.copy_picker.is_some()
    }

    pub(crate) fn copy_picker_loading(&self) -> bool {
        self.copy_picker
            .as_ref()
            .is_some_and(|state| state.is_loading)
    }

    pub(crate) fn copy_picker_preview_active(&self) -> bool {
        self.copy_picker
            .as_ref()
            .is_some_and(|state| state.preview.is_some())
    }

    pub(crate) fn open_copy_picker_loading(&mut self) {
        self.copy_picker = Some(CopyPickerState::default());
        self.close_composer_attached_ui();
    }

    pub(crate) fn apply_copy_picker_payload(&mut self, payload: SessionTreePayload) {
        let Some(mut state) = self.copy_picker.take() else {
            return;
        };
        let current_row_id = payload.current_row_id;
        let previous_rows = std::mem::take(&mut state.rows);
        state.rows = payload
            .rows
            .into_iter()
            .filter_map(CopyPickerRow::from_session_tree_row)
            .collect();
        state.is_loading = false;
        state.error = None;
        state.preview = None;
        state.remap_selected_rows_from_previous_rows(&previous_rows);
        if !state.select_row_by_id(current_row_id.as_deref()) {
            state.select_latest_row();
        }
        self.copy_picker = Some(state);
    }

    pub(crate) fn show_copy_picker_error(&mut self, message: &str) {
        let Some(mut state) = self.copy_picker.take() else {
            return;
        };
        state.is_loading = false;
        state.error = Some(message.to_string());
        self.copy_picker = Some(state);
    }

    pub(crate) fn handle_copy_picker_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.copy_picker.is_none() {
            return OverlayInputResult::Ignored;
        }

        if self.copy_picker_preview_active() {
            return self.handle_copy_picker_preview_key(key);
        }

        if let Some(format) = copy_picker_format_for_key(key) {
            return OverlayInputResult::from_effect(self.copy_picker_copy_effect(format));
        }

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.copy_picker = None;
                OverlayInputResult::Handled
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                self.move_copy_picker_selection(ListNavigationDirection::Previous);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                self.move_copy_picker_selection(ListNavigationDirection::Next);
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(state) = self.copy_picker.as_mut() {
                    state.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(state) = self.copy_picker.as_mut() {
                    state.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Tab if key.modifiers.is_empty() => {
                if let Some(state) = self.copy_picker.as_mut() {
                    state.toggle_selected_row();
                }
                OverlayInputResult::Handled
            }
            _ if is_copy_picker_select_all_shortcut(key) => {
                if let Some(state) = self.copy_picker.as_mut() {
                    state.select_all_or_invert();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_copy_picker_preview();
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    pub(crate) fn handle_copy_picker_mouse_down(
        &mut self,
        button: MouseButton,
        _column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.copy_picker_active() {
            return OverlayInputResult::Ignored;
        }

        if button != MouseButton::Left || self.copy_picker_preview_active() {
            return OverlayInputResult::Handled;
        }

        let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        else {
            return OverlayInputResult::Handled;
        };
        let page_size = fullscreen_list_page_size_for_height(self.height);
        if let Some(state) = self.copy_picker.as_mut() {
            state.select_visible_row(page_size, visible_offset);
        }
        OverlayInputResult::Handled
    }

    fn move_copy_picker_selection(&mut self, direction: ListNavigationDirection) {
        if let Some(state) = self.copy_picker.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn move_copy_picker_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        self.move_copy_picker_selection(direction);
    }

    pub(crate) fn move_copy_picker_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .copy_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.transcript_preview.overlay.scroll_offset = copy_picker_preview_page_offset(
                &mut preview.transcript_preview.transcript,
                content_height,
                preview.transcript_preview.overlay.scroll_offset,
                direction,
            );
            preview.transcript_preview.is_following_bottom = false;
        }
    }

    fn copy_picker_copy_effect(&mut self, format: CopyPickerTextFormat) -> Option<AppEffect> {
        let payload = self
            .copy_picker
            .as_ref()
            .and_then(|state| state.copy_payload(format));
        match payload {
            Some(payload) => Some(AppEffect::CopySelection(payload)),
            None => {
                self.show_toast(ToastSeverity::Info, COPY_PICKER_EMPTY_TOAST);
                None
            }
        }
    }

    fn open_copy_picker_preview(&mut self) {
        let preview_target = {
            let Some(state) = self.copy_picker.as_ref() else {
                return;
            };
            let Some(row) = state.selected_row() else {
                return;
            };
            let transcript = self
                .transcript_from_session_tree_preview_replay_with_tool_activity_render_mode(
                    row.preview_replay(),
                    ToolActivityRenderMode::DebugDetailed,
                );
            (state.selected, transcript)
        };

        let (row_index, mut transcript) = preview_target;
        transcript.set_reasoning_render_mode(ReasoningRenderMode::Detailed);
        let content_height = self.transcript_overlay_content_height();
        let mut transcript_preview = TranscriptPreviewState::following_bottom(transcript);
        transcript_preview.sync_follow_bottom(content_height);
        let preview = CopyPickerPreviewState {
            row_index,
            transcript_preview,
        };

        if let Some(state) = self.copy_picker.as_mut() {
            state.preview = Some(preview);
        }
    }

    pub(crate) fn sync_copy_picker_preview_follow_bottom(&mut self) {
        let content_height = self.transcript_overlay_content_height();
        let Some(preview) = self
            .copy_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
            .filter(|preview| preview.transcript_preview.is_following_bottom)
        else {
            return;
        };
        preview
            .transcript_preview
            .sync_follow_bottom(content_height);
    }

    pub(crate) fn sync_copy_picker_preview_width(&mut self, width: u16) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .copy_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.transcript_preview.set_width(width, content_height);
        }
    }

    pub(crate) fn sync_copy_picker_preview_palette(&mut self, palette: TerminalPalette) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .copy_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview
                .transcript_preview
                .set_palette(palette, content_height);
        }
    }

    fn close_copy_picker_preview(&mut self) {
        if let Some(state) = self.copy_picker.as_mut() {
            state.preview = None;
        }
    }

    fn handle_copy_picker_preview_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if let Some(format) = copy_picker_format_for_key(key) {
            return OverlayInputResult::from_effect(self.copy_picker_copy_effect(format));
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_copy_picker_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_copy_picker_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_copy_picker_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 模态覆盖层吞掉未绑定输入，防止落入 composer
        }
    }

    #[cfg(test)]
    pub(crate) fn copy_picker_row_ids_for_test(&self) -> Vec<&str> {
        self.copy_picker
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

    #[cfg(test)]
    pub(crate) fn copy_picker_selected_row_indices_for_test(&self) -> Vec<usize> {
        self.copy_picker
            .as_ref()
            .map(|state| state.selected_row_indices.iter().copied().collect())
            .unwrap_or_default()
    }
}

fn copy_picker_format_for_key(key: KeyEvent) -> Option<CopyPickerTextFormat> {
    if is_uppercase_letter_shortcut(key, 'C') {
        Some(CopyPickerTextFormat::Raw)
    } else if key.code == KeyCode::Char('c') && key.modifiers.is_empty() {
        Some(CopyPickerTextFormat::Display)
    } else {
        None
    }
}

fn is_copy_picker_select_all_shortcut(key: KeyEvent) -> bool {
    is_uppercase_letter_shortcut(key, 'A')
}

fn is_uppercase_letter_shortcut(key: KeyEvent, uppercase_letter: char) -> bool {
    let lowercase_letter = uppercase_letter.to_ascii_lowercase();
    let is_shifted_char = matches!(key.code, KeyCode::Char(character) if character == uppercase_letter || character == lowercase_letter)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    let is_uppercase_without_modifier =
        key.code == KeyCode::Char(uppercase_letter) && key.modifiers.is_empty();

    is_shifted_char || is_uppercase_without_modifier
}
