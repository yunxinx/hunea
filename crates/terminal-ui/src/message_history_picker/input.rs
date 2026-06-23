use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::session::{MessageHistoryRow, SessionLoadRequestId};

use crate::{
    AppEffect, Model,
    fullscreen_list_chrome::{
        fullscreen_list_body_visible_offset_for_row, fullscreen_list_page_size_for_height,
    },
    list_selection::ListNavigationDirection,
    overlay_input_result::OverlayInputResult,
    text_search::is_picker_search_text_key,
    time::current_unix_timestamp_ms,
};

use super::MessageHistoryPickerState;

impl Model {
    pub(crate) fn message_history_picker_active(&self) -> bool {
        self.message_history_picker.is_some()
    }

    pub(crate) fn open_message_history_picker_loading(&mut self) -> SessionLoadRequestId {
        self.open_message_history_picker_loading_at(current_unix_timestamp_ms())
    }

    pub(crate) fn open_message_history_picker_loading_at(
        &mut self,
        opened_at_ms: i64,
    ) -> SessionLoadRequestId {
        let request_id = self.next_session_load_request_id();
        self.message_history_picker = Some(MessageHistoryPickerState {
            is_loading: true,
            opened_at_ms,
            pending_request_id: Some(request_id),
            ..MessageHistoryPickerState::default()
        });
        self.close_composer_attached_ui();
        request_id
    }

    pub(crate) fn apply_message_history_picker_rows(
        &mut self,
        request_id: SessionLoadRequestId,
        rows: Vec<MessageHistoryRow>,
    ) {
        let Some(mut state) = self.message_history_picker.take() else {
            return;
        };
        if !state.is_loading || state.pending_request_id != Some(request_id) {
            self.message_history_picker = Some(state);
            return;
        }
        state.rows = rows;
        state.is_loading = false;
        state.pending_request_id = None;
        state.error = None;
        state.apply_filter();
        state.select_latest_row();
        self.message_history_picker = Some(state);
    }

    pub(crate) fn show_message_history_picker_error(
        &mut self,
        request_id: SessionLoadRequestId,
        message: &str,
    ) {
        let Some(mut state) = self.message_history_picker.take() else {
            return;
        };
        if !state.is_loading || state.pending_request_id != Some(request_id) {
            self.message_history_picker = Some(state);
            return;
        }
        state.is_loading = false;
        state.pending_request_id = None;
        state.error = Some(message.to_string());
        state.rows.clear();
        state.selected = 0;
        state.selected_row_id = None;
        self.message_history_picker = Some(state);
    }

    pub(crate) fn message_history_picker_load_request_matches(
        &self,
        request_id: SessionLoadRequestId,
    ) -> bool {
        self.message_history_picker
            .as_ref()
            .is_some_and(|state| state.is_loading && state.pending_request_id == Some(request_id))
    }

    #[cfg(test)]
    pub(crate) fn message_history_picker_pending_request_id_for_test(
        &self,
    ) -> Option<SessionLoadRequestId> {
        self.message_history_picker
            .as_ref()
            .and_then(|state| state.pending_request_id)
    }

    pub(crate) fn move_message_history_picker_selection_by_delta(&mut self, delta: isize) {
        let Some(direction) = ListNavigationDirection::from_delta(delta) else {
            return;
        };
        if let Some(state) = self.message_history_picker.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn handle_message_history_picker_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        if self.message_history_picker.is_none() {
            return OverlayInputResult::Ignored;
        }

        if self.message_history_picker_preview_active() {
            return self.handle_message_history_picker_preview_key(key);
        }

        let is_searching = self
            .message_history_picker
            .as_ref()
            .is_some_and(|state| state.is_searching);

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                if let Some(state) = self.message_history_picker.as_mut()
                    && state.exit_search()
                {
                    return OverlayInputResult::Handled;
                }
                self.message_history_picker = None;
                OverlayInputResult::Handled
            }
            KeyCode::Char(character) if is_searching && is_picker_search_text_key(&key) => {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.push_search_character(character);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Backspace if key.modifiers.is_empty() => {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.backspace_search();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.clear_search();
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.is_searching = true;
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char('c') if key.modifiers.is_empty() => {
                OverlayInputResult::from_effect(self.message_history_picker_copy_effect())
            }
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.move_selection(ListNavigationDirection::Previous);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.move_selection(ListNavigationDirection::Next);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.move_page(ListNavigationDirection::Previous, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = fullscreen_list_page_size_for_height(self.height);
                if let Some(state) = self.message_history_picker.as_mut() {
                    state.move_page(ListNavigationDirection::Next, page_size);
                }
                OverlayInputResult::Handled
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.open_message_history_picker_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Enter => self.handle_message_history_picker_enter(),
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn message_history_picker_copy_effect(&mut self) -> Option<AppEffect> {
        let payload = self
            .message_history_picker
            .as_ref()
            .and_then(MessageHistoryPickerState::copy_payload_full_text);
        payload.map(AppEffect::CopySelection)
    }

    fn handle_message_history_picker_enter(&mut self) -> OverlayInputResult {
        let Some(state) = self.message_history_picker.as_ref() else {
            return OverlayInputResult::Handled;
        };
        if state.is_loading || state.error.is_some() {
            return OverlayInputResult::Handled;
        }
        let recalled = match state.selected_row() {
            Some(row) => row.text.clone(),
            None => {
                self.message_history_picker = None;
                return OverlayInputResult::Handled;
            }
        };

        let draft = self.composer_text().to_string();

        self.message_history_picker = None;

        let record_effect = if runtime_domain::session::should_record_message_history_text(&draft) {
            self.blind_recall.push_local_entry(draft.clone());
            Some(AppEffect::RecordMessageHistory { text: draft })
        } else {
            None
        };

        self.apply_message_history_picker_recall(&recalled);

        OverlayInputResult::from_effect(record_effect)
    }

    fn apply_message_history_picker_recall(&mut self, text: &str) {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        if text.is_empty() {
            self.composer_mut().clear_for_edit();
        } else {
            self.composer_mut()
                .reset_text_and_move_to_end(text.to_string());
        }
        self.blind_recall.apply_recalled_text(text);

        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    pub(crate) fn handle_message_history_picker_mouse_down(
        &mut self,
        button: MouseButton,
        _column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if !self.message_history_picker_active() {
            return OverlayInputResult::Ignored;
        }

        if button != MouseButton::Left || self.message_history_picker_preview_active() {
            return OverlayInputResult::Handled;
        }

        let Some(visible_offset) = fullscreen_list_body_visible_offset_for_row(self.height, row)
        else {
            return OverlayInputResult::Handled;
        };
        let page_size = fullscreen_list_page_size_for_height(self.height);
        if let Some(state) = self.message_history_picker.as_mut() {
            state.select_visible_row(page_size, visible_offset);
        }
        OverlayInputResult::Handled
    }

    pub(crate) fn can_open_message_history_picker_via_ctrl_r(&self) -> bool {
        self.top_modal_layer().is_none()
            && !self.model_panel_active()
            && !self.tool_approval_panel_active()
            && !self.command_panel_active()
            && !self.file_picker_active()
    }
}
