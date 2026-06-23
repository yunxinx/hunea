use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::session::{TranscriptReplayItem, TranscriptReplayRole};

use crate::{
    Model,
    overlay_input_result::OverlayInputResult,
    tool_result::ToolActivityRenderMode,
    transcript::{ReasoningRenderMode, preview_page_offset as message_history_preview_page_offset},
    transcript_preview::TranscriptPreviewState,
};

use super::state::MessageHistoryPickerPreviewState;

impl Model {
    pub(crate) fn message_history_picker_preview_active(&self) -> bool {
        self.message_history_picker
            .as_ref()
            .is_some_and(|state| state.preview.is_some())
    }

    pub(crate) fn move_message_history_picker_preview_page(&mut self, direction: isize) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.transcript_preview.overlay.scroll_offset = message_history_preview_page_offset(
                &mut preview.transcript_preview.transcript,
                content_height,
                preview.transcript_preview.overlay.scroll_offset,
                direction,
            );
            preview.transcript_preview.is_following_bottom = false;
        }
    }

    pub(crate) fn sync_message_history_picker_preview_follow_bottom(&mut self) {
        let content_height = self.transcript_overlay_content_height();
        let Some(preview) = self
            .message_history_picker
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

    pub(crate) fn sync_message_history_picker_preview_width(&mut self, width: u16) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview.transcript_preview.set_width(width, content_height);
        }
    }

    pub(crate) fn sync_message_history_picker_preview_palette(
        &mut self,
        palette: crate::theme::TerminalPalette,
    ) {
        let content_height = self.transcript_overlay_content_height();
        if let Some(preview) = self
            .message_history_picker
            .as_mut()
            .and_then(|state| state.preview.as_mut())
        {
            preview
                .transcript_preview
                .set_palette(palette, content_height);
        }
    }

    pub(super) fn open_message_history_picker_preview(&mut self) {
        let preview_target = {
            let Some(state) = self.message_history_picker.as_ref() else {
                return;
            };
            if state.is_loading || state.error.is_some() {
                return;
            }
            let Some(row) = state.selected_row() else {
                return;
            };
            let transcript = self.transcript_from_replay_items_with_tool_activity_render_mode(
                [TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::User,
                    content: row.text.clone(),
                }],
                ToolActivityRenderMode::DebugDetailed,
            );
            (state.selected, transcript)
        };

        let (row_index, mut transcript) = preview_target;
        transcript.set_reasoning_render_mode(ReasoningRenderMode::Detailed);
        let content_height = self.transcript_overlay_content_height();
        let mut transcript_preview = TranscriptPreviewState::following_bottom(transcript);
        transcript_preview.sync_follow_bottom(content_height);
        let preview = MessageHistoryPickerPreviewState {
            row_index,
            transcript_preview,
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
}
