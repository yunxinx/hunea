use std::{path::PathBuf, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

use crate::runtime::models::ModelSelection;

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
    StartAcpSession {
        agent_id: String,
    },
    SendAcpPrompt {
        agent_id: String,
        prompt: String,
    },
    RespondAcpPermission {
        request_id: String,
        option_id: Option<String>,
    },
    PersistSelectedModel {
        selection: ModelSelection,
    },
}

/// `AppEvent` 描述 TUI 模型可处理的外部事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEvent {
    Key(KeyEvent),
    Paste(String),
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
    HistoryScrollIndicatorTimeout {
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
    AcpPermissionRequested {
        request_id: String,
        title: Option<String>,
        allow_option_id: Option<String>,
        reject_option_id: Option<String>,
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
            AppEvent::Paste(text) => self.handle_paste(&text),
            AppEvent::Resized { width, height } => {
                self.handle_resize(width, height);
                None
            }
            AppEvent::MouseWheel { delta_lines } => {
                self.cancel_exit_confirmation();
                let before_document_viewport_y = self.document_runtime.viewport_y;
                let before_composer_viewport_y = self.composer.viewport_offset();
                let before_follow_bottom = self.document_runtime.follow_bottom;
                let before_manual_document_scroll = self.document_runtime.manual_scroll;
                let had_pending_click = self.pending_composer_cursor_click.active;
                self.scroll_document_by(delta_lines);
                if self.document_runtime.viewport_y != before_document_viewport_y
                    || self.composer.viewport_offset() != before_composer_viewport_y
                    || self.document_runtime.follow_bottom != before_follow_bottom
                    || self.document_runtime.manual_scroll != before_manual_document_scroll
                {
                    self.clear_pending_composer_cursor_click();
                    if had_pending_click {
                        self.reset_selection_click();
                    }
                    self.show_history_scroll_indicator();
                }
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
            AppEvent::HistoryScrollIndicatorTimeout { token } => {
                self.dismiss_history_scroll_indicator(token);
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
            AppEvent::AcpPermissionRequested {
                request_id,
                title,
                allow_option_id,
                reject_option_id,
            } => {
                self.show_acp_permission_request(
                    request_id,
                    title,
                    allow_option_id,
                    reject_option_id,
                );
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

        self.clear_history_scroll_indicator();
        if !(key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)) {
            self.cancel_exit_confirmation();
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.ctrl_c_clears_input && !self.composer_text().is_empty() {
                self.cancel_exit_confirmation();
                return self.handle_composer_clear_input();
            }
            if self.exit_confirmation_active(std::time::Instant::now()) {
                self.mark_quitting();
            } else {
                self.show_exit_confirmation();
            }
            return None;
        }

        if let Some(effect) = self.handle_model_panel_key(key) {
            return effect;
        }

        if let Some(effect) = self.handle_acp_panel_key(key) {
            return effect;
        }

        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self
                .maybe_prepare_external_editor_launch()
                .map(AppEffect::LaunchExternalEditor);
        }

        if let Some(effect) = self.handle_acp_permission_key(key) {
            return effect;
        }

        if let Some(effect) = self.handle_command_panel_key(key) {
            return effect;
        }

        if key.code == KeyCode::Enter {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                if self.swap_enter_and_send {
                    return self.handle_composer_send();
                }
                return self.handle_composer_insert_newline();
            }

            if self.swap_enter_and_send {
                return self.handle_composer_insert_newline();
            }
            return self.handle_composer_send();
        }

        if key.code == KeyCode::Char('j') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.swap_enter_and_send {
                return self.handle_composer_send();
            }
            return self.handle_composer_insert_newline();
        }

        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
            && self.document_runtime.manual_scroll
        {
            return None;
        }

        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            let old_value = self.composer_text().to_string();
            let old_line = self.composer.line();
            let old_column = self.composer.column();
            let direction = if key.code == KeyCode::PageUp { -1 } else { 1 };
            if self.composer_mut().handle_page_key(direction) {
                self.sync_composer_height();
                self.document_runtime.follow_bottom = self.composer.viewport_offset()
                    == self.composer.bottom_viewport_offset()
                    && self.composer_at_bottom_follow_anchor();
                self.document_runtime.manual_scroll = false;
                self.clear_manual_document_scroll_restore_target();
                if self.document_runtime.follow_bottom {
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
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_composer_insert_newline(&mut self) -> Option<AppEffect> {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().insert_newline();
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_paste(&mut self, text: &str) -> Option<AppEffect> {
        if text.is_empty() {
            return None;
        }

        self.cancel_exit_confirmation();
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut()
            .insert_text(&normalize_pasted_text(text));
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_composer_clear_input(&mut self) -> Option<AppEffect> {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().clear();
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_composer_send(&mut self) -> Option<AppEffect> {
        let content = self.composer_text().to_string();
        if content.trim().is_empty() {
            return None;
        }
        if self.requires_model_selection
            && self.selected_model.is_none()
            && self.selected_acp_agent.is_none()
        {
            self.show_transient_status_notice("Select a model before sending");
            return None;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        self.transcript_mut().append_message_with_style_mode(
            Sender::User,
            content.clone(),
            style_mode,
        );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.composer_mut().clear();
        self.sync_command_panel_navigation();
        self.sync_external_editor_helper_after_draft_change(&content);
        self.sync_composer_height();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.selected_acp_agent
            .clone()
            .map(|agent_id| AppEffect::SendAcpPrompt {
                agent_id,
                prompt: content,
            })
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        self.cancel_exit_confirmation();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let previous_width = self.width;
        let had_pending_click = self.pending_composer_cursor_click.active;

        if self.selection_runtime.selection.is_active() {
            self.invalidate_selection_for_reflow();
        }
        self.set_window(width, height);
        if had_pending_click {
            self.clear_pending_composer_cursor_click();
            self.reset_selection_click();
        }
        self.sync_external_editor_helper_after_resize(previous_width);
        self.sync_command_panel_navigation();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }
}

fn normalize_pasted_text(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(character) = chars.next() {
        if character == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
            continue;
        }

        normalized.push(character);
    }

    normalized
}
