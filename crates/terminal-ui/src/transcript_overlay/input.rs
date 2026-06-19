use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{Model, overlay_input_result::OverlayInputResult};

impl Model {
    /// `handle_active_transcript_overlay_key` 统一 transcript 覆盖层及 message revisit 子模式的输入优先级。
    pub(crate) fn handle_active_transcript_overlay_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        let result = self.handle_message_revisit_overlay_key(key);
        if !result.is_ignored() {
            return result;
        }
        self.handle_transcript_overlay_key(key)
    }

    /// `handle_transcript_overlay_global_key` 处理无需覆盖层已激活的全局 transcript 快捷键。
    pub(crate) fn handle_transcript_overlay_global_key(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        if is_transcript_overlay_toggle_key(key) {
            self.toggle_transcript_overlay();
            return OverlayInputResult::Handled;
        }
        OverlayInputResult::Ignored
    }

    /// `handle_transcript_overlay_key` 处理覆盖层激活时的键盘事件。
    pub(crate) fn handle_transcript_overlay_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        let global_result = self.handle_transcript_overlay_global_key(key);
        if !global_result.is_ignored() {
            return global_result;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.close_transcript_overlay();
            return OverlayInputResult::Handled;
        }

        let content_height = self.transcript_overlay_content_height();
        let metrics_index = self.transcript.progressive_item_metrics_index();
        let max_offset =
            self.transcript_overlay_max_offset_for_index(&metrics_index, content_height);

        let Some(overlay) = self.transcript_overlay.as_mut() else {
            return OverlayInputResult::Ignored;
        };

        match key.code {
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(1);
                OverlayInputResult::Handled
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                overlay.scroll_offset = (overlay.scroll_offset + 1).min(max_offset);
                OverlayInputResult::Handled
            }
            KeyCode::PageUp => {
                let page = content_height.saturating_sub(1).max(1);
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(page);
                OverlayInputResult::Handled
            }
            KeyCode::PageDown => {
                let page = content_height.saturating_sub(1).max(1);
                overlay.scroll_offset = (overlay.scroll_offset + page).min(max_offset);
                OverlayInputResult::Handled
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                let half_page = content_height / 2;
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(half_page.max(1));
                OverlayInputResult::Handled
            }
            KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                let half_page = content_height / 2;
                overlay.scroll_offset = (overlay.scroll_offset + half_page.max(1)).min(max_offset);
                OverlayInputResult::Handled
            }
            KeyCode::Home => {
                overlay.scroll_offset = 0;
                OverlayInputResult::Handled
            }
            KeyCode::End => {
                overlay.scroll_offset = max_offset;
                OverlayInputResult::Handled
            }
            KeyCode::Esc if key.modifiers.is_empty() => {
                self.close_transcript_overlay();
                OverlayInputResult::Handled
            }
            _ => OverlayInputResult::Handled, // 覆盖层激活时，消费所有其它按键，防止落入 composer
        }
    }
}

fn is_transcript_overlay_toggle_key(key: KeyEvent) -> bool {
    key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL)
}
