use std::{path::PathBuf, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

use super::{
    ExternalEditorLaunch, Model, Sender,
    theme::{TerminalPalette, palette_from_background, terminal_default_palette},
};

/// `STARTUP_PROBE_TIMEOUT` 是启动阶段等待主题探测结果的最长时长。
pub const STARTUP_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// `AppEffect` 表示 runner 需要在模型外执行的一次副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    LaunchExternalEditor(ExternalEditorLaunch),
    CopySelection(String),
}

/// `AppEvent` 描述 TUI 模型可处理的外部事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Resized {
        width: u16,
        height: u16,
    },
    MouseWheel {
        delta_lines: isize,
    },
    MouseDown {
        button: MouseButton,
        column: u16,
        row: u16,
    },
    MouseUp {
        button: MouseButton,
        column: u16,
        row: u16,
    },
    MouseDrag {
        button: MouseButton,
        column: u16,
        row: u16,
    },
    DetectedPalette {
        palette: TerminalPalette,
        has_dark_background: bool,
    },
    ForegroundColorHint {
        is_dark: bool,
    },
    StatusNoticeTimeout {
        token: usize,
    },
    ExternalEditorHelperTimeout {
        token: usize,
    },
    ExternalEditorFinished {
        draft_path: PathBuf,
        original_draft: String,
        failed: bool,
    },
    SelectionAutoScrollTick {
        token: usize,
    },
    SelectionCopyCompleted {
        success: bool,
    },
    StartupReadyTimeout,
}

impl Model {
    /// `update` 根据事件推进模型状态。
    pub fn update(&mut self, event: AppEvent) -> Option<AppEffect> {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Resized { width, height } => {
                self.handle_resize(width, height);
                None
            }
            AppEvent::MouseWheel { delta_lines } => {
                self.cancel_exit_confirmation();
                self.scroll_document_by(delta_lines);
                None
            }
            AppEvent::MouseDown {
                button,
                column,
                row,
            } => self.handle_mouse_down(button, column, row),
            AppEvent::MouseUp {
                button,
                column,
                row,
            } => self.handle_mouse_up(button, column, row),
            AppEvent::MouseDrag {
                button,
                column,
                row,
            } => self.handle_mouse_drag(button, column, row),
            AppEvent::DetectedPalette {
                palette,
                has_dark_background,
            } => {
                self.set_palette(palette, has_dark_background);
                None
            }
            AppEvent::ForegroundColorHint { is_dark } => {
                if !self.has_palette() {
                    self.set_palette(palette_from_background(!is_dark, None), !is_dark);
                }
                None
            }
            AppEvent::StatusNoticeTimeout { token } => {
                self.dismiss_status_notice(token);
                None
            }
            AppEvent::ExternalEditorHelperTimeout { token } => {
                self.dismiss_external_editor_helper(token);
                None
            }
            AppEvent::ExternalEditorFinished {
                draft_path,
                original_draft,
                failed,
            } => {
                self.apply_external_editor_finished(&draft_path, &original_draft, failed);
                None
            }
            AppEvent::SelectionAutoScrollTick { token } => {
                self.handle_selection_auto_scroll_tick(token);
                None
            }
            AppEvent::SelectionCopyCompleted { success } => {
                self.handle_selection_copy_completed(success);
                None
            }
            AppEvent::StartupReadyTimeout => {
                if !self.has_palette() {
                    self.set_palette(terminal_default_palette(), false);
                }
                None
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<AppEffect> {
        if !(key.kind.is_press() || key.kind.is_repeat()) {
            return None;
        }

        if !(key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
            self.cancel_exit_confirmation();
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.exit_confirmation_active(std::time::Instant::now()) {
                self.mark_quitting();
            } else {
                self.show_exit_confirmation();
            }
            return None;
        }

        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self
                .maybe_prepare_external_editor_launch()
                .map(AppEffect::LaunchExternalEditor);
        }

        if key.code == KeyCode::Enter {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                let old_value = self.composer_text().to_string();
                let old_line = self.composer.line();
                let old_column = self.composer.column();
                self.composer_mut().insert_newline();
                self.sync_external_editor_helper_after_draft_change(&old_value);
                self.sync_composer_height();
                self.sync_document_viewport_after_composer_interaction(
                    &old_value, old_line, old_column,
                );
                return None;
            }

            let content = self.composer_text().to_string();
            if content.trim().is_empty() {
                return None;
            }

            let preserved_anchor = if self.manual_document_scroll {
                self.current_document_viewport_anchor()
            } else {
                None
            };
            let style_mode = self.style_mode;
            self.transcript_mut().append_message_with_style_mode(
                Sender::User,
                content.clone(),
                style_mode,
            );
            self.refresh_status_line_after_transcript_change();
            self.sync_transcript_render();
            self.composer_mut().clear();
            self.sync_external_editor_helper_after_draft_change(&content);
            self.sync_composer_height();
            self.follow_bottom = true;
            self.sync_document_viewport_after_transcript_refresh(preserved_anchor);
            return None;
        }

        if key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let old_value = self.composer_text().to_string();
            let old_line = self.composer.line();
            let old_column = self.composer.column();
            self.composer_mut().insert_newline();
            self.sync_external_editor_helper_after_draft_change(&old_value);
            self.sync_composer_height();
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
            return None;
        }

        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) && self.manual_document_scroll {
            return None;
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
            return None;
        }

        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().handle_key(key);
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        self.cancel_exit_confirmation();
        let preserved_anchor = if self.manual_document_scroll {
            self.current_document_viewport_anchor()
        } else {
            None
        };
        let previous_width = self.width;

        if self.selection.active {
            self.invalidate_selection_for_reflow();
        }
        self.set_window(width, height);
        self.sync_external_editor_helper_after_resize(previous_width);
        self.sync_document_viewport_after_transcript_refresh(preserved_anchor);
    }
}
