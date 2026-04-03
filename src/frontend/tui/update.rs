use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{
    Model, Sender,
    theme::{TerminalPalette, palette_from_background, terminal_default_palette},
};

/// `STARTUP_PROBE_TIMEOUT` 是启动阶段等待主题探测结果的最长时长。
pub const STARTUP_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// `AppEvent` 描述 TUI 模型可处理的外部事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Resized {
        width: u16,
        height: u16,
    },
    MouseWheel {
        delta_lines: isize,
    },
    DetectedPalette {
        palette: TerminalPalette,
        has_dark_background: bool,
    },
    ForegroundColorHint {
        is_dark: bool,
    },
    StartupReadyTimeout,
}

impl Model {
    /// `update` 根据事件推进模型状态。
    pub fn update(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Resized { width, height } => self.handle_resize(width, height),
            AppEvent::MouseWheel { delta_lines } => self.scroll_document_by(delta_lines),
            AppEvent::DetectedPalette {
                palette,
                has_dark_background,
            } => self.set_palette(palette, has_dark_background),
            AppEvent::ForegroundColorHint { is_dark } => {
                if !self.has_palette() {
                    self.set_palette(palette_from_background(!is_dark, None), !is_dark);
                }
            }
            AppEvent::StartupReadyTimeout => {
                if !self.has_palette() {
                    self.set_palette(terminal_default_palette(), false);
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if !(key.kind.is_press() || key.kind.is_repeat()) {
            return;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.mark_quitting();
            return;
        }

        if key.code == KeyCode::Enter {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                let old_value = self.composer_text().to_string();
                let old_line = self.composer.line();
                let old_column = self.composer.column();
                self.composer_mut().insert_newline();
                self.sync_composer_height();
                self.sync_document_viewport_after_composer_interaction(
                    &old_value, old_line, old_column,
                );
                return;
            }

            let content = self.composer_text().to_string();
            if content.trim().is_empty() {
                return;
            }

            let preserved_anchor = if self.manual_document_scroll {
                self.current_document_viewport_anchor()
            } else {
                None
            };
            self.transcript_mut().append_message(Sender::User, content);
            self.sync_transcript_render();
            self.composer_mut().clear();
            self.sync_composer_height();
            self.follow_bottom = true;
            self.sync_document_viewport_after_transcript_refresh(preserved_anchor);
            return;
        }

        if key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let old_value = self.composer_text().to_string();
            let old_line = self.composer.line();
            let old_column = self.composer.column();
            self.composer_mut().insert_newline();
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
            return;
        }

        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) && self.manual_document_scroll {
            return;
        }

        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let old_value = self.composer_text().to_string();
            let old_line = self.composer.line();
            let old_column = self.composer.column();
            let direction = if key.code == KeyCode::PageUp { -1 } else { 1 };
            if self.composer_mut().handle_page_key(direction) {
                self.sync_composer_height();
                self.follow_bottom = self.composer.viewport_offset()
                    == self.composer.bottom_viewport_offset()
                    && self.composer_at_bottom_follow_anchor();
                self.manual_document_scroll = false;
                self.clear_manual_document_scroll_restore_target();
                if self.follow_bottom {
                    self.sync_document_viewport_to_bottom();
                } else {
                    self.sync_document_viewport_for_composer_page();
                }
                self.sync_document_viewport_after_composer_interaction(
                    &old_value, old_line, old_column,
                );
            }
            return;
        }

        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().handle_key(key);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        let preserved_anchor = if self.manual_document_scroll {
            self.current_document_viewport_anchor()
        } else {
            None
        };

        self.set_window(width, height);
        self.sync_document_viewport_after_transcript_refresh(preserved_anchor);
    }
}
