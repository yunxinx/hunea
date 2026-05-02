use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::frontend::tui::{AppEffect, Model, Sender, transcript::TranscriptItem};

#[cfg(test)]
mod tests;

const BACKTRACK_HINT: &str = "Press Esc again to edit previous message";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BacktrackState {
    pub(crate) primed: bool,
    pub(crate) selected_item_index: Option<usize>,
    pub(crate) overlay_preview_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BacktrackSelection {
    item_index: usize,
    prefill: String,
}

impl Model {
    pub(crate) fn handle_backtrack_main_esc_key(&mut self) -> Option<Option<AppEffect>> {
        if self.backtrack.primed && self.current_status_notice_text() != BACKTRACK_HINT {
            self.reset_backtrack_state();
        }

        if self.selected_acp_agent.is_some()
            || self.stream_activity.is_some()
            || !self.composer_text().is_empty()
        {
            self.reset_backtrack_state();
            return None;
        }

        if !self.has_backtrack_target() {
            self.reset_backtrack_state();
            return None;
        }

        if !self.backtrack.primed {
            self.backtrack.primed = true;
            self.backtrack.selected_item_index = None;
            self.show_transient_status_notice(BACKTRACK_HINT);
            return Some(None);
        }

        self.clear_backtrack_notice();
        self.open_transcript_overlay();
        self.backtrack.overlay_preview_active = true;
        self.select_latest_backtrack_target();
        Some(None)
    }

    pub(crate) fn handle_backtrack_overlay_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<AppEffect>> {
        if !self.backtrack.overlay_preview_active {
            return None;
        }

        match key.code {
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transcript_overlay();
                Some(None)
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.close_transcript_overlay();
                Some(None)
            }
            KeyCode::Char('q') if key.modifiers.is_empty() => {
                self.close_transcript_overlay();
                Some(None)
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_transcript_overlay();
                Some(None)
            }
            KeyCode::Left if key.modifiers.is_empty() => {
                self.step_backtrack_selection(-1);
                Some(None)
            }
            KeyCode::Right if key.modifiers.is_empty() => {
                self.step_backtrack_selection(1);
                Some(None)
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                let selection = self.current_backtrack_selection();
                self.close_transcript_overlay();
                if let Some(selection) = selection {
                    self.apply_native_backtrack_selection(selection);
                }
                Some(None)
            }
            _ => self.handle_transcript_overlay_key(key),
        }
    }

    pub(crate) fn reset_backtrack_state(&mut self) {
        self.backtrack = BacktrackState::default();
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = None;
        }
        self.clear_backtrack_notice();
    }

    pub(crate) fn reset_backtrack_state_for_status_notice_timeout(&mut self, token: usize) {
        if self.backtrack.primed
            && self.notice_state.status_token == token
            && self.current_status_notice_text() == BACKTRACK_HINT
        {
            self.reset_backtrack_state();
        }
    }

    fn has_backtrack_target(&self) -> bool {
        !self.backtrack_user_item_indices().is_empty()
    }

    fn select_latest_backtrack_target(&mut self) {
        if let Some(item_index) = self.backtrack_user_item_indices().last().copied() {
            self.apply_backtrack_item_selection(item_index);
        }
    }

    fn step_backtrack_selection(&mut self, direction: isize) {
        let positions = self.backtrack_user_item_indices();
        if positions.is_empty() {
            self.apply_backtrack_no_selection();
            return;
        }

        let last_position = positions.len().saturating_sub(1);
        let current_position = self
            .backtrack
            .selected_item_index
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

        self.apply_backtrack_item_selection(positions[next_position]);
    }

    fn apply_backtrack_item_selection(&mut self, item_index: usize) {
        self.backtrack.selected_item_index = Some(item_index);
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = Some(item_index);
        }
        self.scroll_transcript_overlay_item_into_view(item_index);
    }

    fn apply_backtrack_no_selection(&mut self) {
        self.backtrack.selected_item_index = None;
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.highlight_item_index = None;
        }
    }

    fn current_backtrack_selection(&self) -> Option<BacktrackSelection> {
        let item_index = self.backtrack.selected_item_index?;
        let items = self.transcript.items_snapshot();
        let item = items.get(item_index)?;
        match item.as_ref() {
            TranscriptItem::Message(message) if message.sender() == Sender::User => {
                Some(BacktrackSelection {
                    item_index,
                    prefill: message.source_content().to_string(),
                })
            }
            TranscriptItem::Hero(_)
            | TranscriptItem::Message(_)
            | TranscriptItem::Reasoning(_)
            | TranscriptItem::System(_)
            | TranscriptItem::ToolResult(_) => None,
        }
    }

    fn apply_native_backtrack_selection(&mut self, selection: BacktrackSelection) {
        if self.selected_acp_agent.is_some() {
            return;
        }

        let old_value = self.composer_text().to_string();
        if self.selection_runtime.selection.is_active() {
            self.invalidate_selection_for_reflow();
        }
        if !self
            .transcript_mut()
            .truncate_before_item(selection.item_index)
        {
            return;
        }

        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.composer_mut()
            .replace_text_and_move_to_end(selection.prefill);
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.document_runtime.follow_bottom = true;
        self.document_runtime.manual_scroll = false;
        self.clear_manual_document_scroll_restore_target();
        self.sync_document_viewport_to_bottom();
    }

    fn backtrack_user_item_indices(&self) -> Vec<usize> {
        self.transcript
            .items_snapshot()
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item.as_ref() {
                TranscriptItem::Message(message) if message.sender() == Sender::User => Some(index),
                TranscriptItem::Hero(_)
                | TranscriptItem::Message(_)
                | TranscriptItem::Reasoning(_)
                | TranscriptItem::System(_)
                | TranscriptItem::ToolResult(_) => None,
            })
            .collect()
    }

    fn clear_backtrack_notice(&mut self) {
        if self.current_status_notice_text() == BACKTRACK_HINT {
            self.clear_status_notice();
        }
    }
}
