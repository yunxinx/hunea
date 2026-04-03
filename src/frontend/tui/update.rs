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
            AppEvent::Resized { width, height } => self.set_window(width, height),
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
            let content = self.composer_text().trim().to_string();
            if !content.is_empty() {
                self.transcript_mut().append_message(Sender::User, content);
                self.composer_mut().clear();
            }
            return;
        }

        self.composer_mut().handle_key(key);
    }
}
