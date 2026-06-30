use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    AppEffect, Model, Sender, overlay_input_result::OverlayInputResult, toast::ToastSeverity,
    transcript::TranscriptItem,
};

#[cfg(test)]
mod tests;

const MESSAGE_REVISIT_HINT: &str = "Press Esc again to edit previous message";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct MessageRevisitState {
    pub(crate) is_armed: bool,
    pub(crate) selected_message_index: Option<usize>,
    pub(crate) is_overlay_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageRevisitSelection {
    item_index: usize,
    prefill: String,
    retained_user_turns: usize,
}

impl Model {
    pub(crate) fn open_coarse_rewind_from_command(&mut self) -> Option<AppEffect> {
        self.reset_message_revisit_state();
        if self.stream_activity.is_some() || !self.composer_text().is_empty() {
            return None;
        }
        if !self.has_message_revisit_target() {
            self.show_toast(ToastSeverity::Info, "No previous user message");
            return None;
        }

        self.clear_message_revisit_notice();
        self.open_transcript_overlay();
        self.message_revisit.is_overlay_active = true;
        self.select_latest_message_revisit_target();
        None
    }

    pub(crate) fn handle_message_revisit_main_esc_key(&mut self) -> OverlayInputResult {
        if self.message_revisit.is_armed
            && self.current_status_notice_text() != MESSAGE_REVISIT_HINT
        {
            self.reset_message_revisit_state();
        }

        if self.stream_activity.is_some() || !self.composer_text().is_empty() {
            self.reset_message_revisit_state();
            return OverlayInputResult::Ignored;
        }

        if !self.has_message_revisit_target() {
            self.reset_message_revisit_state();
            return OverlayInputResult::Ignored;
        }

        if !self.message_revisit.is_armed {
            self.message_revisit.is_armed = true;
            self.message_revisit.selected_message_index = None;
            self.show_transient_status_notice(MESSAGE_REVISIT_HINT);
            return OverlayInputResult::Handled;
        }

        if matches!(self.esc_rewind_mode, crate::EscRewindMode::Entry) {
            self.clear_message_revisit_notice();
            self.reset_message_revisit_state();
            return OverlayInputResult::Effect(AppEffect::OpenEntryRewind);
        }

        self.clear_message_revisit_notice();
        self.open_transcript_overlay();
        self.message_revisit.is_overlay_active = true;
        self.select_latest_message_revisit_target();
        OverlayInputResult::Handled
    }

    pub(crate) fn handle_message_revisit_overlay_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        if !self.message_revisit.is_overlay_active {
            return OverlayInputResult::Ignored;
        }

        match key.code {
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transcript_overlay();
                OverlayInputResult::Handled
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transcript_overlay();
                OverlayInputResult::Handled
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_transcript_overlay();
                OverlayInputResult::Handled
            }
            KeyCode::Left if key.modifiers.is_empty() => {
                self.step_message_revisit_selection(-1);
                OverlayInputResult::Handled
            }
            KeyCode::Right if key.modifiers.is_empty() => {
                self.step_message_revisit_selection(1);
                OverlayInputResult::Handled
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let selection = self.current_message_revisit_selection();
                self.close_transcript_overlay();
                if let Some(selection) = selection {
                    return OverlayInputResult::from_effect(
                        self.apply_message_revisit_selection(selection),
                    );
                }
                OverlayInputResult::Handled
            }
            _ => self.handle_transcript_overlay_key(key),
        }
    }

    pub(crate) fn reset_message_revisit_state(&mut self) {
        self.message_revisit = MessageRevisitState::default();
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = None;
        }
        self.clear_message_revisit_notice();
    }

    pub(crate) fn reset_message_revisit_state_for_status_notice_timeout(&mut self, token: usize) {
        if self.message_revisit.is_armed
            && self.notice_state.status_token == token
            && self.current_status_notice_text() == MESSAGE_REVISIT_HINT
        {
            self.reset_message_revisit_state();
        }
    }

    fn has_message_revisit_target(&self) -> bool {
        !self.message_revisit_user_message_indices().is_empty()
    }

    fn select_latest_message_revisit_target(&mut self) {
        if let Some(item_index) = self.message_revisit_user_message_indices().last().copied() {
            self.apply_message_revisit_item_selection(item_index);
        }
    }

    fn step_message_revisit_selection(&mut self, direction: isize) {
        let positions = self.message_revisit_user_message_indices();
        if positions.is_empty() {
            self.clear_message_revisit_selection();
            return;
        }

        let last_position = positions.len().saturating_sub(1);
        let current_position = self
            .message_revisit
            .selected_message_index
            .and_then(|item_index| {
                positions
                    .iter()
                    .position(|candidate| *candidate == item_index)
            })
            .unwrap_or(last_position);
        let next_position = if direction < 0 {
            current_position.saturating_sub(1)
        } else {
            current_position.saturating_add(1).min(last_position)
        };

        self.apply_message_revisit_item_selection(positions[next_position]);
    }

    fn apply_message_revisit_item_selection(&mut self, item_index: usize) {
        self.message_revisit.selected_message_index = Some(item_index);
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = Some(item_index);
        }
        self.scroll_transcript_overlay_item_into_view(item_index);
    }

    fn clear_message_revisit_selection(&mut self) {
        self.message_revisit.selected_message_index = None;
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = None;
        }
    }

    fn current_message_revisit_selection(&self) -> Option<MessageRevisitSelection> {
        let item_index = self.message_revisit.selected_message_index?;
        let items = self.transcript.items_snapshot();
        let item = items.get(item_index)?;
        match item.as_ref() {
            TranscriptItem::Message(message) if message.sender() == Sender::User => {
                Some(MessageRevisitSelection {
                    item_index,
                    prefill: message.source_content().to_string(),
                    retained_user_turns: self
                        .message_revisit_user_message_indices()
                        .into_iter()
                        .filter(|candidate| *candidate < item_index)
                        .count(),
                })
            }
            TranscriptItem::StartupBanner(_)
            | TranscriptItem::Message(_)
            | TranscriptItem::Reasoning(_)
            | TranscriptItem::System(_)
            | TranscriptItem::ToolResult(_)
            | TranscriptItem::WorkDuration(_)
            | TranscriptItem::FinalBodyDivider(_) => None,
        }
    }

    fn apply_message_revisit_selection(
        &mut self,
        selection: MessageRevisitSelection,
    ) -> Option<AppEffect> {
        let old_value = self.composer_text().to_string();
        if self.selection_runtime.selection.is_active() {
            self.invalidate_selection_for_reflow();
        }
        if !self
            .transcript_mut()
            .truncate_before_item(selection.item_index)
        {
            return None;
        }

        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.composer_mut()
            .reset_text_and_move_to_end(selection.prefill);
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.document_runtime.follow_bottom = true;
        self.document_runtime.manual_scroll = false;
        self.clear_manual_document_scroll_restore_target();
        self.sync_document_viewport_to_bottom();
        Some(AppEffect::TruncateConversation {
            retained_user_turns: selection.retained_user_turns,
        })
    }

    fn message_revisit_user_message_indices(&self) -> Vec<usize> {
        self.transcript
            .items_snapshot()
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item.as_ref() {
                TranscriptItem::Message(message) if message.sender() == Sender::User => Some(index),
                TranscriptItem::StartupBanner(_)
                | TranscriptItem::Message(_)
                | TranscriptItem::Reasoning(_)
                | TranscriptItem::System(_)
                | TranscriptItem::ToolResult(_)
                | TranscriptItem::WorkDuration(_)
                | TranscriptItem::FinalBodyDivider(_) => None,
            })
            .collect()
    }

    fn clear_message_revisit_notice(&mut self) {
        if self.current_status_notice_text() == MESSAGE_REVISIT_HINT {
            self.clear_status_notice();
        }
    }
}
