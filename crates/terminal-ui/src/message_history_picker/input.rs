use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use session_store::MessageHistoryRow;

use crate::{
    Model, fullscreen_list_chrome::fullscreen_list_page_size_for_height,
    list_selection::ListNavigationDirection, overlay_input_result::OverlayInputResult,
};

use super::MessageHistoryPickerState;

impl Model {
    pub(crate) fn message_history_picker_active(&self) -> bool {
        self.message_history_picker.is_some()
    }

    pub(crate) fn open_message_history_picker_loading(&mut self) {
        self.open_message_history_picker_loading_at(current_unix_time_ms());
    }

    pub(crate) fn open_message_history_picker_loading_at(&mut self, opened_at_ms: i64) {
        self.message_history_picker = Some(MessageHistoryPickerState {
            is_loading: true,
            opened_at_ms,
            ..MessageHistoryPickerState::default()
        });
        self.close_composer_attached_ui();
    }

    pub(crate) fn apply_message_history_picker_rows(&mut self, rows: Vec<MessageHistoryRow>) {
        let mut state = self.message_history_picker.take().unwrap_or_default();
        state.rows = rows;
        state.is_loading = false;
        state.error = None;
        state.select_latest_row();
        self.message_history_picker = Some(state);
    }

    pub(crate) fn show_message_history_picker_error(&mut self, message: &str) {
        let mut state = self.message_history_picker.take().unwrap_or_default();
        state.is_loading = false;
        state.error = Some(message.to_string());
        state.rows.clear();
        state.selected = 0;
        self.message_history_picker = Some(state);
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

        match key.code {
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.message_history_picker = None;
                OverlayInputResult::Handled
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
            KeyCode::Enter => OverlayInputResult::Handled,
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn can_open_message_history_picker_via_ctrl_r(&self) -> bool {
        self.top_modal_layer().is_none()
            && !self.model_panel_active()
            && !self.tool_approval_panel_active()
            && !self.command_panel_active()
            && !self.file_picker_active()
    }
}

fn current_unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
