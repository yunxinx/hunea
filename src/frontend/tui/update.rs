use std::{path::PathBuf, time::Duration};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};

use crate::runtime::{
    acp::{AcpPrompt, build_acp_prompt_from_composer_text},
    model_catalog::ModelSelection,
    native::models::ProviderSyncRequest,
    native::{ChatMessage, NativeAgentRequest},
};

use super::{
    ExternalEditorLaunch, Model, Sender,
    exit_confirmation::EXIT_CONFIRMATION_PROMPT,
    theme::{TerminalPalette, palette_from_background, terminal_default_palette},
};

/// `STARTUP_PROBE_TIMEOUT` 是启动阶段等待主题探测结果的最长时长。
pub const STARTUP_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// `AppEffect` 表示 runner 需要在模型外执行的一次副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    LaunchExternalEditor(ExternalEditorLaunch),
    CopySelection(String),
    ResetRuntimeSession,
    StartAcpSession {
        agent_id: String,
    },
    SendAcpPrompt {
        agent_id: String,
        prompt: AcpPrompt,
    },
    RespondAcpPermission {
        request_id: String,
        option_id: Option<String>,
        is_rejection: bool,
        rejected_tool_call_id: Option<String>,
    },
    SetAcpModel {
        config_id: Option<String>,
        value: String,
    },
    SendNativeAgent {
        request: NativeAgentRequest,
    },
    InterruptCurrentTurn,
    PersistSelectedModel {
        selection: ModelSelection,
    },
    RefreshModelProvider {
        request: ProviderSyncRequest,
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
        allow_always_option_id: Option<String>,
        reject_option_id: Option<String>,
        reject_always_option_id: Option<String>,
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
            AppEvent::Paste(text) => {
                if self.transcript_overlay_active() {
                    self.cancel_exit_confirmation();
                    None
                } else {
                    self.handle_paste(&text)
                }
            }
            AppEvent::Resized { width, height } => {
                self.handle_resize(width, height);
                None
            }
            AppEvent::MouseWheel { delta_lines } => {
                self.cancel_exit_confirmation();
                if self.transcript_overlay_active() {
                    return None;
                }
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
            } => {
                if self.transcript_overlay_active() {
                    return None;
                }
                self.handle_mouse_down(button, column, row)
            }
            AppEvent::MouseUp {
                button,
                column,
                row,
            } => {
                if self.transcript_overlay_active() {
                    return None;
                }
                self.handle_mouse_up(button, column, row)
            }
            AppEvent::MouseDrag {
                button,
                column,
                row,
            } => {
                if self.transcript_overlay_active() {
                    return None;
                }
                self.handle_mouse_drag(button, column, row)
            }
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
                self.reset_backtrack_state_for_status_notice_timeout(token);
                self.dismiss_status_notice(token);
                self.reset_chat_interrupt_esc_count();
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
                allow_always_option_id,
                reject_option_id,
                reject_always_option_id,
            } => {
                self.show_acp_permission_request(
                    request_id,
                    title,
                    allow_option_id,
                    allow_always_option_id,
                    reject_option_id,
                    reject_always_option_id,
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

        let is_plain_esc = key.code == KeyCode::Esc && key.modifiers.is_empty();
        let is_ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        self.clear_history_scroll_indicator();
        if self.transcript_overlay_active() {
            self.cancel_exit_confirmation();
            if let Some(effect) = self.handle_backtrack_overlay_key(key) {
                return effect;
            }
            if let Some(effect) = self.handle_transcript_overlay_key(key) {
                return effect;
            }
        }

        if !is_plain_esc {
            self.reset_backtrack_state();
        }

        let was_canceling_exit_confirmation =
            is_plain_esc && self.current_status_notice_text() == EXIT_CONFIRMATION_PROMPT;
        if !is_ctrl_c {
            self.cancel_exit_confirmation();
            if was_canceling_exit_confirmation {
                return None;
            }
        }

        if is_ctrl_c {
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

        if let Some(effect) = self.handle_tool_approval_panel_key(key) {
            return effect;
        }

        if let Some(effect) = self.handle_transcript_overlay_key(key) {
            return effect;
        }

        if is_plain_esc && let Some(effect) = self.handle_chat_interrupt_key() {
            return Some(effect);
        } else if key.code != KeyCode::Esc {
            self.reset_chat_interrupt_esc_count();
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

        if let Some(effect) = self.handle_command_panel_key(key) {
            return effect;
        }

        if let Some(effect) = self.handle_file_picker_key(key) {
            return effect;
        }

        if is_plain_esc && let Some(effect) = self.handle_backtrack_main_esc_key() {
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
                self.sync_file_picker_state();
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
        let file_picker_was_active = self.file_picker_active();
        let file_picker_manual_viewport_state = (file_picker_was_active
            && self.document_runtime.manual_scroll)
            .then(|| self.current_document_viewport_state());
        self.composer_mut().handle_key(key);
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        let file_picker_closed = file_picker_was_active && !self.file_picker_active();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        if self.composer_text() != old_value
            && file_picker_closed
            && let Some(state) = file_picker_manual_viewport_state.as_ref()
        {
            if self.selection_runtime.selection.is_active() {
                self.invalidate_selection_for_reflow();
            }
            self.sync_document_viewport_for_viewport_state(state);
        } else {
            self.sync_document_viewport_after_composer_interaction(
                &old_value, old_line, old_column,
            );
        }
        None
    }

    fn handle_chat_interrupt_key(&mut self) -> Option<AppEffect> {
        if !self.chat_turn_interruptible() {
            self.reset_chat_interrupt_esc_count();
            return None;
        }

        self.chat_interrupt_esc_count = self.chat_interrupt_esc_count.saturating_add(1);
        if self.chat_interrupt_esc_count >= self.esc_interrupt_presses {
            self.reset_chat_interrupt_esc_count();
            return Some(AppEffect::InterruptCurrentTurn);
        }

        let remaining = self
            .esc_interrupt_presses
            .saturating_sub(self.chat_interrupt_esc_count);
        if remaining == 1 {
            self.show_transient_status_notice("Press Esc again to interrupt");
        } else {
            self.show_transient_status_notice(&format!(
                "Press Esc {remaining} more times to interrupt"
            ));
        }
        None
    }

    fn chat_turn_interruptible(&self) -> bool {
        self.stream_activity.is_some()
    }

    pub(crate) fn reset_chat_interrupt_esc_count(&mut self) {
        self.chat_interrupt_esc_count = 0;
    }

    fn handle_composer_insert_newline(&mut self) -> Option<AppEffect> {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        self.composer_mut().insert_newline();
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
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
        self.sync_file_picker_state();
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
        self.sync_file_picker_state();
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
        if self.stream_activity.is_some() && self.selected_acp_agent.is_none() {
            self.show_transient_status_notice("Chat request is already running");
            return None;
        }
        if self.selected_acp_agent.is_none()
            && let Some(selection) = self.selected_model.clone()
            && !self.validate_native_agent_selection(&selection)
        {
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
        self.sync_file_picker_state();
        self.sync_external_editor_helper_after_draft_change(&content);
        self.sync_composer_height();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        if let Some(agent_id) = self.selected_acp_agent.clone() {
            self.show_stream_activity(
                self.acp_current_model
                    .clone()
                    .unwrap_or_else(|| agent_id.clone()),
            );
            let identity = self
                .acp_agent_identities
                .get(&agent_id)
                .cloned()
                .unwrap_or_default();
            let prompt =
                build_acp_prompt_from_composer_text(&content, self.acp_prompt_root(), &identity);
            return Some(AppEffect::SendAcpPrompt { agent_id, prompt });
        }

        let selection = self.selected_model.clone()?;
        self.native_agent_request_for_selection(&selection)
            .map(|request| AppEffect::SendNativeAgent { request })
    }

    fn acp_prompt_root(&self) -> PathBuf {
        let trimmed = self.current_dir.trim();
        if trimmed.is_empty() {
            return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        }
        if trimmed == "~" {
            return home_dir().unwrap_or_else(|| PathBuf::from("."));
        }
        if let Some(rest) = trimmed.strip_prefix("~/") {
            return home_dir()
                .map(|home| home.join(rest))
                .unwrap_or_else(|| PathBuf::from(trimmed));
        }

        let path = PathBuf::from(trimmed);
        if path.is_absolute() {
            path
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| PathBuf::from(trimmed))
        }
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        self.cancel_exit_confirmation();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let transcript_overlay_anchor = self.capture_transcript_overlay_scroll_anchor();
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
        self.sync_file_picker_state();
        self.sync_composer_height();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.restore_transcript_overlay_scroll_anchor(transcript_overlay_anchor);
    }
}

impl Model {
    fn native_agent_request_for_selection(
        &mut self,
        selection: &ModelSelection,
    ) -> Option<NativeAgentRequest> {
        let Some(provider) = self
            .model_catalog
            .enabled_provider_by_id(&selection.provider_id)
        else {
            self.show_transient_status_notice("Selected provider is not available");
            return None;
        };
        let Some(native_runtime) = provider.native_runtime() else {
            self.show_transient_status_notice("Selected provider is not native");
            return None;
        };
        Some(NativeAgentRequest::new(
            selection.provider_id.clone(),
            native_runtime.kind,
            selection.model_id.clone(),
            native_runtime.base_url.clone(),
            native_runtime.api_key.clone(),
            native_runtime.api_key_env.clone(),
            self.agent_messages_from_transcript(),
        ))
    }

    fn validate_native_agent_selection(&mut self, selection: &ModelSelection) -> bool {
        let Some(provider) = self
            .model_catalog
            .enabled_provider_by_id(&selection.provider_id)
        else {
            self.show_transient_status_notice("Selected provider is not available");
            return false;
        };

        let Some(native_runtime) = provider.native_runtime() else {
            self.show_transient_status_notice("Selected provider is not native");
            return false;
        };

        if native_runtime.kind.uses_openai_compatible_endpoint()
            && native_runtime
                .base_url
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            self.show_transient_status_notice("Selected provider has no base_url");
            return false;
        }

        true
    }

    fn agent_messages_from_transcript(&self) -> Vec<ChatMessage> {
        self.transcript
            .source_messages()
            .into_iter()
            .map(|(sender, content)| match sender {
                Sender::User => ChatMessage::user(content),
                Sender::Assistant => ChatMessage::assistant(content),
            })
            .collect()
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

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}
