use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{AppEffect, Model};

impl Model {
    /// `handle_transcript_overlay_key` 处理覆盖层激活时的键盘事件。
    /// 返回 `Some(None)` 表示捕获事件；`None` 表示不处理，继续向下传递。
    pub(crate) fn handle_transcript_overlay_key(
        &mut self,
        key: KeyEvent,
    ) -> Option<Option<AppEffect>> {
        // Ctrl+T 始终切换覆盖层（无论当前是否激活）
        if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.toggle_transcript_overlay();
            return Some(None);
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.close_transcript_overlay();
            return Some(None);
        }

        let content_height = self.transcript_overlay_content_height();
        let metrics_index = self.transcript.progressive_item_metrics_index();
        let max_offset =
            self.transcript_overlay_max_offset_for_index(&metrics_index, content_height);

        let overlay = self.transcript_overlay.as_mut()?;

        match key.code {
            KeyCode::Up | KeyCode::Char('k') if key.modifiers.is_empty() => {
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(1);
                Some(None)
            }
            KeyCode::Down | KeyCode::Char('j') if key.modifiers.is_empty() => {
                overlay.scroll_offset = (overlay.scroll_offset + 1).min(max_offset);
                Some(None)
            }
            KeyCode::PageUp => {
                let page = content_height.saturating_sub(1).max(1);
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(page);
                Some(None)
            }
            KeyCode::PageDown => {
                let page = content_height.saturating_sub(1).max(1);
                overlay.scroll_offset = (overlay.scroll_offset + page).min(max_offset);
                Some(None)
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                let half_page = content_height / 2;
                overlay.scroll_offset = overlay.scroll_offset.saturating_sub(half_page.max(1));
                Some(None)
            }
            KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => {
                let half_page = content_height / 2;
                overlay.scroll_offset = (overlay.scroll_offset + half_page.max(1)).min(max_offset);
                Some(None)
            }
            KeyCode::Home => {
                overlay.scroll_offset = 0;
                Some(None)
            }
            KeyCode::End => {
                overlay.scroll_offset = max_offset;
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
            _ => Some(None), // 覆盖层激活时，消费所有其它按键，防止落入 composer
        }
    }
}
