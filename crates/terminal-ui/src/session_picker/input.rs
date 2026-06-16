use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime_domain::session::SessionPickerRow;

use crate::{AppEffect, Model};

use super::{SessionPickerState, session_picker_page_size_for_height};

impl Model {
    pub(crate) fn move_session_picker_selection(&mut self, direction: isize) {
        if let Some(state) = self.session_picker.as_mut() {
            state.move_selection(direction);
        }
    }

    pub(crate) fn session_picker_active(&self) -> bool {
        self.session_picker.is_some()
    }

    pub(crate) fn open_session_picker_loading(&mut self) {
        self.open_session_picker_loading_at(current_unix_time_ms());
    }

    pub(crate) fn open_session_picker_loading_at(&mut self, opened_at_ms: i64) {
        self.session_picker = Some(SessionPickerState {
            is_loading: true,
            opened_at_ms,
            ..SessionPickerState::default()
        });
    }

    pub(crate) fn apply_session_picker_rows(&mut self, rows: Vec<SessionPickerRow>) {
        let mut state = self.session_picker.take().unwrap_or_default();
        state.rows = rows;
        state.is_loading = false;
        state.error = None;
        state.apply_filter();
        self.session_picker = Some(state);
    }

    pub(crate) fn handle_session_picker_key(&mut self, key: KeyEvent) -> Option<Option<AppEffect>> {
        if !self.session_picker_active() {
            return None;
        }

        let is_searching = self
            .session_picker
            .as_ref()
            .is_some_and(|state| state.is_searching);

        match key.code {
            KeyCode::Esc => {
                if let Some(state) = self.session_picker.as_mut()
                    && state.exit_search()
                {
                    return Some(None);
                }
                self.session_picker = None;
                Some(None)
            }
            KeyCode::Char(character) if is_searching && is_session_picker_search_text_key(&key) => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.push_search_character(character);
                }
                Some(None)
            }
            KeyCode::Up => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(-1);
                }
                Some(None)
            }
            KeyCode::Down => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(1);
                }
                Some(None)
            }
            KeyCode::Left => {
                let page_size = session_picker_page_size_for_height(self.height);
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(-1, page_size);
                }
                Some(None)
            }
            KeyCode::Right => {
                let page_size = session_picker_page_size_for_height(self.height);
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(1, page_size);
                }
                Some(None)
            }
            KeyCode::Char('k') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(-1);
                }
                Some(None)
            }
            KeyCode::Char('j') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_selection(1);
                }
                Some(None)
            }
            KeyCode::Char('h') if key.modifiers.is_empty() => {
                let page_size = session_picker_page_size_for_height(self.height);
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(-1, page_size);
                }
                Some(None)
            }
            KeyCode::Char('l') if key.modifiers.is_empty() => {
                let page_size = session_picker_page_size_for_height(self.height);
                if let Some(state) = self.session_picker.as_mut() {
                    state.move_page(1, page_size);
                }
                Some(None)
            }
            KeyCode::Backspace => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.backspace_search();
                }
                Some(None)
            }
            KeyCode::Char('u')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(state) = self.session_picker.as_mut() {
                    state.clear_search();
                }
                Some(None)
            }
            KeyCode::Enter => {
                let selected_session_id = self
                    .session_picker
                    .as_ref()
                    .and_then(SessionPickerState::selected_row)
                    .map(|row| row.session_id.clone());
                if let Some(session_id) = selected_session_id {
                    self.session_picker = None;
                    return Some(Some(AppEffect::ResumeSession { session_id }));
                }
                Some(None)
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                if let Some(state) = self.session_picker.as_mut() {
                    state.is_searching = true;
                }
                Some(None)
            }
            KeyCode::Char(' ') if key.modifiers.is_empty() => {
                let selected_session_id = self
                    .session_picker
                    .as_ref()
                    .and_then(SessionPickerState::selected_row)
                    .map(|row| row.session_id.clone());
                selected_session_id
                    .map(|session_id| Some(AppEffect::OpenSessionPreview { session_id }))
                    .map(Some)
                    .unwrap_or(Some(None))
            }
            _ => Some(None),
        }
    }
}

fn is_session_picker_search_text_key(key: &KeyEvent) -> bool {
    let KeyCode::Char(character) = key.code else {
        return false;
    };
    !character.is_ascii_control()
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

fn current_unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(i64::MAX)
}
