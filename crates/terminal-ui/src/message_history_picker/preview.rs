use crossterm::event::{KeyCode, KeyEvent};

use crate::{Model, overlay_input_result::OverlayInputResult, transcript::wrap_plain_text};

use super::state::MessageHistoryPickerPreviewState;

const MESSAGE_HISTORY_PREVIEW_HORIZONTAL_PADDING: usize = 2;

impl Model {
    pub(crate) fn message_history_picker_preview_active(&self) -> bool {
        self.message_history_picker
            .as_ref()
            .is_some_and(|state| state.preview.is_some())
    }

    pub(crate) fn move_message_history_picker_preview_page(&mut self, direction: isize) {
        let page_size = self.message_history_picker_preview_content_height();
        let line_count = self
            .message_history_picker_preview_wrapped_lines()
            .map_or(0, |lines| lines.len());
        if let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            let max_offset = line_count.saturating_sub(page_size);
            let delta = direction.signum() * isize::try_from(page_size).unwrap_or(0);
            let next = isize::try_from(preview.scroll_offset)
                .unwrap_or(0)
                .saturating_add(delta);
            let max_offset_i = isize::try_from(max_offset).unwrap_or(0);
            preview.scroll_offset = usize::try_from(next.clamp(0, max_offset_i)).unwrap_or(0);
        }
    }

    pub(crate) fn sync_message_history_picker_preview_width(&mut self, width: u16) {
        let row_index = self
            .message_history_picker
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .map(|preview| preview.row_index);
        let Some(row_index) = row_index else {
            return;
        };
        let wrap_width = message_history_preview_wrap_width(width);
        let wrapped_line_count = self
            .message_history_picker
            .as_ref()
            .and_then(|state| state.row(row_index))
            .map(|row| wrap_plain_text(&row.text, wrap_width, 0).len())
            .unwrap_or(0);
        let page_size = self.message_history_picker_preview_content_height();
        let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        else {
            return;
        };
        let max_offset = wrapped_line_count.saturating_sub(page_size);
        preview.scroll_offset = preview.scroll_offset.min(max_offset);
    }

    pub(super) fn open_message_history_picker_preview(&mut self) {
        let row_index = {
            let Some(state) = self.message_history_picker.as_ref() else {
                return;
            };
            if state.is_loading || state.error.is_some() {
                return;
            }
            let Some(row_index) = state.selected_row_index() else {
                return;
            };
            if state.row(row_index).is_none() {
                return;
            }
            row_index
        };

        let preview = MessageHistoryPickerPreviewState {
            row_index,
            scroll_offset: 0,
        };

        if let Some(state) = self.message_history_picker.as_mut() {
            state.preview = Some(preview);
        }
    }

    fn close_message_history_picker_preview(&mut self) {
        if let Some(state) = self.message_history_picker.as_mut() {
            state.preview = None;
        }
    }

    pub(super) fn handle_message_history_picker_preview_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        if key.code == KeyCode::Char('c') && key.modifiers.is_empty() {
            return OverlayInputResult::from_effect(self.message_history_picker_copy_effect());
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char(' ') if key.modifiers.is_empty() => {
                self.close_message_history_picker_preview();
                OverlayInputResult::Handled
            }
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') if key.modifiers.is_empty() => {
                self.move_message_history_picker_preview_page(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right | KeyCode::Down | KeyCode::Char('l') if key.modifiers.is_empty() => {
                self.move_message_history_picker_preview_page(1);
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled,
        }
    }

    pub(crate) fn message_history_picker_preview_content_height(&self) -> usize {
        usize::from(self.height.saturating_sub(2).max(1))
    }

    pub(crate) fn message_history_picker_preview_wrapped_lines(&self) -> Option<Vec<String>> {
        let state = self.message_history_picker.as_ref()?;
        let preview = state.preview.as_ref()?;
        let row = state.row(preview.row_index)?;
        let wrap_width = message_history_preview_wrap_width(self.width);
        Some(wrap_plain_text(&row.text, wrap_width, 0))
    }
}

pub(super) fn message_history_preview_wrap_width(window_width: u16) -> usize {
    usize::from(window_width)
        .saturating_sub(MESSAGE_HISTORY_PREVIEW_HORIZONTAL_PADDING * 2)
        .max(1)
}
