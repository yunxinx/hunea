use std::{path::PathBuf, time::Duration};

use super::{
    ExternalEditorLaunch, Model, Sender,
    composer::{
        ComposerSourceMessage, selection_end_char_for_line_anchor,
        selection_start_char_for_line_anchor,
    },
    document::DocumentAnchorRegion,
    exit_confirmation::EXIT_CONFIRMATION_PROMPT,
    modal_layer::ModalLayer,
    overlay_input_result::OverlayInputResult,
    terminal_text::sanitize_terminal_text,
    theme::{TerminalPalette, palette_from_background, terminal_default_palette},
    toast::ToastSeverity,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::{
    model_catalog::{ModelSelection, ProviderSyncRequest},
    session::{
        ConversationTurnRequest, MessageHistoryEntryId, PendingMessageHistoryEntry, RuntimeTarget,
        SessionLoadRequestId,
    },
};

/// `STARTUP_PROBE_TIMEOUT` 是启动阶段等待主题探测结果的最长时长。
pub const STARTUP_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// `AppEffect` 表示 runner 需要在模型外执行的一次副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEffect {
    LaunchExternalEditor(ExternalEditorLaunch),
    CopySelection(String),
    OpenCopyPicker,
    OpenContextBudget,
    OpenMessageHistory,
    BeginPromptAssemblyEdit,
    ApplyPromptAssemblyEditMutation {
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    },
    ResetRuntimeSession,
    RespondRuntimePermission {
        target: RuntimeTarget,
        request_id: String,
        option_id: Option<String>,
    },
    OpenResumePicker,
    OpenSessionPreview {
        session_id: String,
    },
    ResumeSession {
        session_id: String,
    },
    OpenEntryRewind,
    OpenBranchTree,
    SelectEntryRewind {
        entry_id: String,
        prefill: Option<String>,
    },
    OpenBranchPreview {
        request_id: SessionLoadRequestId,
        branch_row_id: String,
    },
    SwitchBranch {
        leaf_id: String,
    },
    TruncateConversation {
        retained_user_turns: usize,
    },
    SendConversationTurn {
        request: Box<ConversationTurnRequest>,
        /// 发送前已写入盲回溯、需异步落库的正文；相邻重复或未写入时为 `None`。
        record_message_history: Option<PendingMessageHistoryEntry>,
    },
    InterruptCurrentTurn,
    PersistSelectedModel {
        selection: ModelSelection,
    },
    RefreshModelProvider {
        request: ProviderSyncRequest,
    },
    RecordMessageHistory {
        entry_id: MessageHistoryEntryId,
        text: String,
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
    SelectionAutoScrollTick {
        token: usize,
    },
    SelectionCopyCompleted {
        success: bool,
    },
    ToastNoticeTimeout {
        token: usize,
    },
    StartupReadyTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerEnterAction {
    Send,
    InsertNewline,
}

impl Model {
    pub(crate) fn terminal_input_coalescing(&self) -> crate::runner::TerminalInputCoalescing {
        crate::runner::TerminalInputCoalescing {
            has_page_scroll_burst_coalescing: self.modal_has_page_scroll_burst_coalescing(),
        }
    }

    /// `update` 根据事件推进模型状态。
    pub fn update(&mut self, event: AppEvent) -> Option<AppEffect> {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Paste(text) => {
                if self.blocks_composer_input() {
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
                let result = self.handle_overlay_mouse_wheel(delta_lines);
                if !result.is_ignored() {
                    return result.into_effect();
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
                let result = self.handle_overlay_mouse_down(button, column, row);
                if !result.is_ignored() {
                    return result.into_effect();
                }
                self.handle_mouse_down(button, column, row)
            }
            AppEvent::MouseUp {
                button,
                column,
                row,
            } => {
                let result = self.handle_overlay_pointer_passthrough_blocker();
                if !result.is_ignored() {
                    return result.into_effect();
                }
                self.handle_mouse_up(button, column, row)
            }
            AppEvent::MouseDrag {
                button,
                column,
                row,
            } => {
                let result = self.handle_overlay_pointer_passthrough_blocker();
                if !result.is_ignored() {
                    return result.into_effect();
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
                self.reset_message_revisit_state_for_status_notice_timeout(token);
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
            } => self
                .apply_prompt_overlay_external_editor_finished(&draft_path, failed)
                .unwrap_or_else(|| {
                    self.apply_external_editor_finished(&draft_path, &original_draft, failed);
                    None
                }),
            AppEvent::SelectionAutoScrollTick { token } => {
                self.handle_selection_auto_scroll_tick(token);
                None
            }
            AppEvent::SelectionCopyCompleted { success } => {
                self.handle_selection_copy_completed(success);
                None
            }
            AppEvent::ToastNoticeTimeout { token } => {
                self.handle_toast_timeout(token);
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

    fn handle_overlay_mouse_wheel(&mut self, delta_lines: isize) -> OverlayInputResult {
        match self.top_modal_layer() {
            Some(ModalLayer::ToolApprovalFullscreenPreview) => {
                self.scroll_tool_approval_fullscreen_preview_by(delta_lines);
                OverlayInputResult::Handled
            }
            Some(ModalLayer::SessionPreview) => {
                self.move_session_preview_page(delta_lines.signum());
                OverlayInputResult::Handled
            }
            Some(ModalLayer::PromptOverlay) => {
                self.move_prompt_overlay_selection_by_delta(delta_lines.signum());
                OverlayInputResult::Handled
            }
            Some(ModalLayer::SessionPicker) => {
                self.move_session_picker_selection_by_delta(delta_lines.signum());
                OverlayInputResult::Handled
            }
            Some(ModalLayer::CopyPicker) => {
                if self.copy_picker_preview_active() {
                    self.move_copy_picker_preview_page(delta_lines.signum());
                } else {
                    self.move_copy_picker_selection_by_delta(delta_lines.signum());
                }
                OverlayInputResult::Handled
            }
            Some(ModalLayer::EntryTree) => {
                if self.entry_tree_preview_active() {
                    self.move_entry_tree_preview_page(delta_lines.signum());
                } else {
                    self.move_entry_tree_selection_by_delta(delta_lines.signum());
                }
                OverlayInputResult::Handled
            }
            Some(ModalLayer::MessageHistory) => {
                if self.message_history_picker_preview_active() {
                    self.move_message_history_picker_preview_page(delta_lines.signum());
                } else {
                    self.move_message_history_picker_selection_by_delta(delta_lines.signum());
                }
                OverlayInputResult::Handled
            }
            Some(ModalLayer::TranscriptOverlay) => OverlayInputResult::Handled,
            None => OverlayInputResult::Ignored,
        }
    }

    fn handle_overlay_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        match self.top_modal_layer() {
            Some(ModalLayer::CopyPicker) => self.handle_copy_picker_mouse_down(button, column, row),
            Some(ModalLayer::EntryTree) => self.handle_entry_tree_mouse_down(button, column, row),
            Some(ModalLayer::MessageHistory) => {
                self.handle_message_history_picker_mouse_down(button, column, row)
            }
            Some(ModalLayer::PromptOverlay) => {
                self.handle_prompt_overlay_mouse_down(button, column, row)
            }
            Some(_) => OverlayInputResult::Handled,
            None => OverlayInputResult::Ignored,
        }
    }

    fn handle_overlay_pointer_passthrough_blocker(&self) -> OverlayInputResult {
        if self.modal_blocks_pointer_passthrough() {
            OverlayInputResult::Handled
        } else {
            OverlayInputResult::Ignored
        }
    }

    fn handle_modal_layer_key(&mut self, layer: ModalLayer, key: KeyEvent) -> OverlayInputResult {
        match layer {
            ModalLayer::ToolApprovalFullscreenPreview => self.handle_tool_approval_panel_key(key),
            ModalLayer::TranscriptOverlay => self.handle_active_transcript_overlay_key(key),
            ModalLayer::PromptOverlay => self.handle_prompt_overlay_key(key),
            ModalLayer::SessionPreview => self.handle_session_preview_key(key),
            ModalLayer::SessionPicker => self.handle_session_picker_key(key),
            ModalLayer::CopyPicker => self.handle_copy_picker_key(key),
            ModalLayer::EntryTree => self.handle_entry_tree_key(key),
            ModalLayer::MessageHistory => self.handle_message_history_picker_key(key),
        }
    }

    fn handle_top_modal_layer_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        let Some(layer) = self.top_modal_layer() else {
            return OverlayInputResult::Ignored;
        };
        self.handle_modal_layer_key(layer, key)
    }

    fn handle_modal_layer_key_before_global_shortcuts(
        &mut self,
        key: KeyEvent,
    ) -> OverlayInputResult {
        let Some(layer) = self
            .top_modal_layer()
            .filter(|layer| layer.handles_key_before_global_shortcuts())
        else {
            return OverlayInputResult::Ignored;
        };
        self.handle_modal_layer_key(layer, key)
    }

    fn handle_current_modal_or_panel_key(&mut self, key: KeyEvent) -> OverlayInputResult {
        if self.tool_approval_panel_active() {
            return self.handle_tool_approval_panel_key(key);
        }
        if self.context_budget_active() {
            return self.handle_context_budget_key(key);
        }
        self.handle_top_modal_layer_key(key)
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<AppEffect> {
        if !(key.kind.is_press() || key.kind.is_repeat()) {
            return None;
        }

        let is_plain_composer_input = matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if !is_plain_composer_input {
            self.composer_mut().finish_current_undo_group();
        }

        let is_plain_esc = key.code == KeyCode::Esc && key.modifiers.is_empty();
        let is_ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        self.clear_history_scroll_indicator();
        let result = self.handle_modal_layer_key_before_global_shortcuts(key);
        if !result.is_ignored() {
            self.cancel_exit_confirmation();
            return result.into_effect();
        }

        if !is_plain_esc {
            self.reset_message_revisit_state();
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

        let result = self.handle_current_modal_or_panel_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }

        let result = self.handle_transcript_overlay_global_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }

        let result = self.handle_command_panel_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }

        if is_plain_esc && let Some(effect) = self.handle_chat_interrupt_key() {
            return Some(effect);
        } else if key.code != KeyCode::Esc {
            self.reset_chat_interrupt_esc_count();
        }

        let result = self.handle_model_panel_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }

        if key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.prompt_overlay_active() {
                return self.open_prompt_overlay_editor_for_selection();
            }
            return self
                .maybe_prepare_external_editor_launch()
                .map(AppEffect::LaunchExternalEditor);
        }

        let result = self.handle_file_picker_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }
        let result = self.handle_skill_picker_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }
        let result = self.handle_custom_prompt_picker_key(key);
        if !result.is_ignored() {
            return result.into_effect();
        }

        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.can_open_message_history_picker_via_ctrl_r() {
                return Some(AppEffect::OpenMessageHistory);
            }
            return None;
        }

        if is_plain_esc {
            let result = self.handle_message_revisit_main_esc_key();
            if !result.is_ignored() {
                return result.into_effect();
            }
        }

        if let Some(action) = composer_enter_action(key, self.swap_enter_and_send) {
            return match action {
                ComposerEnterAction::Send => self.handle_composer_send(),
                ComposerEnterAction::InsertNewline => self.handle_composer_insert_newline(),
            };
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
                self.sync_composer_attached_picker_state();
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

        if self.try_handle_blind_recall_key(key) {
            return None;
        }

        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        let composer_picker_was_active = self.file_picker_active()
            || self.skill_picker_active()
            || self.custom_prompt_picker_active();
        let file_picker_manual_viewport_state = (composer_picker_was_active
            && self.document_runtime.manual_scroll)
            .then(|| self.current_document_viewport_state());
        self.handle_composer_editing_key(key);
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        let file_picker_closed = composer_picker_was_active
            && !self.file_picker_active()
            && !self.skill_picker_active()
            && !self.custom_prompt_picker_active();
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
        if !self.replace_completed_composer_selection("\n") {
            self.composer_mut().insert_newline();
        }
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn try_handle_blind_recall_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers != KeyModifiers::empty() {
            return false;
        }
        if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
            return false;
        }

        let text = self.composer_text();
        let cursor = self.composer.cursor_char_index();
        if !self.blind_recall.should_handle_navigation(text, cursor) {
            return false;
        }

        let old_value = text.to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();

        let applied = match key.code {
            KeyCode::Up => self.blind_recall.navigate_up(),
            KeyCode::Down => self.blind_recall.navigate_down().is_some(),
            _ => false,
        };

        if !applied {
            return true;
        }

        let apply_text = self
            .blind_recall
            .active_history_text()
            .unwrap_or("")
            .to_string();
        self.apply_blind_recall_text(&apply_text);
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        true
    }

    fn apply_blind_recall_text(&mut self, text: &str) {
        if text.is_empty() {
            self.composer_mut().clear_for_edit();
        } else {
            self.composer_mut()
                .reset_text_and_move_to_end(text.to_string());
        }
    }

    fn handle_composer_editing_key(&mut self, key: KeyEvent) {
        if self.apply_composer_selection_edit_for_key(key) {
            return;
        }

        let clears_completed_selection = composer_navigation_key_clears_selection(key);
        self.composer_mut().handle_key(key);
        if clears_completed_selection {
            self.clear_selection_range();
        }
    }

    fn apply_composer_selection_edit_for_key(&mut self, key: KeyEvent) -> bool {
        match composer_selection_edit_for_key(key) {
            Some(ComposerSelectionEdit::Replace(replacement)) => {
                self.replace_completed_composer_selection(replacement.as_str())
            }
            Some(ComposerSelectionEdit::Delete) => self.replace_completed_composer_selection(""),
            Some(ComposerSelectionEdit::Kill) => self.kill_completed_composer_selection(),
            Some(ComposerSelectionEdit::Yank) => {
                self.replace_completed_composer_selection_with_kill_buffer()
            }
            None => false,
        }
    }

    fn replace_completed_composer_selection(&mut self, replacement: &str) -> bool {
        let Some((start, end)) = self.completed_composer_selection_char_range() else {
            return false;
        };

        if !self
            .composer_mut()
            .replace_char_range(start, end, replacement)
        {
            return false;
        }

        self.clear_selection_range();
        true
    }

    fn kill_completed_composer_selection(&mut self) -> bool {
        let Some((start, end)) = self.completed_composer_selection_char_range() else {
            return false;
        };

        if !self.composer_mut().kill_char_range(start, end) {
            return false;
        }

        self.clear_selection_range();
        true
    }

    fn replace_completed_composer_selection_with_kill_buffer(&mut self) -> bool {
        let Some((start, end)) = self.completed_composer_selection_char_range() else {
            return false;
        };

        if !self
            .composer_mut()
            .replace_char_range_with_kill_buffer(start, end)
        {
            return false;
        }

        self.clear_selection_range();
        true
    }

    fn completed_composer_selection_char_range(&mut self) -> Option<(usize, usize)> {
        if !self.selection_runtime.selection.is_active()
            || self.selection_runtime.selection.is_dragging()
        {
            return None;
        }

        let context = crate::frame_time::FrameRenderContext::capture();
        let layout = self.build_document_layout(context);
        let (start, end) = self
            .selection_runtime
            .selection
            .ordered_points(&layout, context)?;
        let start_anchor = layout.line_anchor_at(start.line(), context)?;
        let end_anchor = layout.line_anchor_at(end.line(), context)?;
        if start_anchor.region != DocumentAnchorRegion::Composer
            || end_anchor.region != DocumentAnchorRegion::Composer
        {
            return None;
        }

        let start_char = selection_start_char_for_line_anchor(
            &self.composer,
            start_anchor.composer,
            start.column(),
        )?;
        let end_char =
            selection_end_char_for_line_anchor(&self.composer, end_anchor.composer, end.column())?;
        (start_char < end_char).then_some((start_char, end_char))
    }

    fn handle_paste(&mut self, text: &str) -> Option<AppEffect> {
        if text.is_empty() {
            return None;
        }

        self.cancel_exit_confirmation();
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        let normalized_text = normalize_pasted_text(text);
        if !self.replace_completed_composer_selection(&normalized_text) {
            self.composer_mut().insert_text(&normalized_text);
        }
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        None
    }

    fn handle_composer_clear_input(&mut self) -> Option<AppEffect> {
        let old_value = self.composer_text().to_string();
        let old_line = self.composer.line();
        let old_column = self.composer.column();
        let record_effect = if !self.composer.has_image_attachments() {
            crate::message_history_recall::commit_message_history(self, &old_value)
        } else {
            None
        };
        self.composer_mut().clear_for_edit();
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&old_value);
        self.sync_composer_height();
        self.sync_document_viewport_after_composer_interaction(&old_value, old_line, old_column);
        record_effect
    }

    fn handle_composer_send(&mut self) -> Option<AppEffect> {
        let content = self.composer_text().to_string();
        if content.trim().is_empty() {
            return None;
        }
        let Some(selection) = self.selected_model.selection().cloned() else {
            self.show_toast(
                ToastSeverity::Error,
                "Please configure a model before trying again",
            );
            return None;
        };
        if self.stream_activity.is_some() {
            self.show_toast(ToastSeverity::Error, "Chat request is already running");
            return None;
        }
        if !self.validate_provider_selection(&selection) {
            return None;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        let source_message = self.composer.source_message();
        let record_message_history = if source_message
            .as_transcript_user_message()
            .attachments
            .is_empty()
        {
            crate::message_history_recall::stage_message_history_recall(self, &content)
        } else {
            None
        };
        self.transcript_mut()
            .append_message_with_style_mode_and_source(
                Sender::User,
                content.clone(),
                style_mode,
                Some(source_message.clone()),
            );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.composer_mut().clear();
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_external_editor_helper_after_draft_change(&content);
        self.sync_composer_height();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.conversation_turn_request_for_selection(&selection, source_message)
            .map(|request| AppEffect::SendConversationTurn {
                request: Box::new(request),
                record_message_history,
            })
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
        self.sync_composer_attached_picker_state();
        self.sync_composer_height();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.restore_transcript_overlay_scroll_anchor(transcript_overlay_anchor);
        self.sync_copy_picker_preview_follow_bottom();
        self.sync_entry_tree_preview_follow_bottom();
    }
}

fn composer_enter_action(key: KeyEvent, swap_enter_and_send: bool) -> Option<ComposerEnterAction> {
    let by_swap_mode = |default_action, swapped_action| {
        if swap_enter_and_send {
            swapped_action
        } else {
            default_action
        }
    };

    // 修饰键精确匹配：未定义的组合（如 Ctrl+Shift+Enter）刻意忽略而非回退到发送，
    // 由 undefined_modified_enter_combination_is_ignored 测试锁定。
    // Ctrl+M 仅在 keyboard enhancement 生效时可区分；legacy 终端把它报告为
    // 无修饰 Enter，走默认发送路径，这是终端协议的固有限制。
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => Some(by_swap_mode(
            ComposerEnterAction::Send,
            ComposerEnterAction::InsertNewline,
        )),
        (KeyCode::Enter, KeyModifiers::SHIFT) | (KeyCode::Char('j'), KeyModifiers::CONTROL) => {
            Some(by_swap_mode(
                ComposerEnterAction::InsertNewline,
                ComposerEnterAction::Send,
            ))
        }
        (KeyCode::Enter, KeyModifiers::ALT) | (KeyCode::Char('m'), KeyModifiers::CONTROL) => {
            Some(ComposerEnterAction::InsertNewline)
        }
        _ => None,
    }
}

impl Model {
    fn conversation_turn_request_for_selection(
        &mut self,
        selection: &ModelSelection,
        message: ComposerSourceMessage,
    ) -> Option<ConversationTurnRequest> {
        let Some(provider) = self
            .model_catalog
            .enabled_provider_by_id(&selection.provider_id)
        else {
            self.show_toast(ToastSeverity::Error, "Selected provider is not available");
            return None;
        };
        let connection = provider.connection();
        Some(ConversationTurnRequest::new_user_source_message(
            selection.provider_id.clone(),
            connection.kind,
            selection.model_id.clone(),
            connection.base_url.clone(),
            connection.api_key.clone(),
            connection.api_key_env.clone(),
            message.into_transcript_user_message(),
        ))
    }

    fn validate_provider_selection(&mut self, selection: &ModelSelection) -> bool {
        let Some(provider) = self
            .model_catalog
            .enabled_provider_by_id(&selection.provider_id)
        else {
            self.show_toast(ToastSeverity::Error, "Selected provider is not available");
            return false;
        };

        let connection = provider.connection();

        if connection.kind.uses_openai_compatible_endpoint()
            && connection
                .base_url
                .as_ref()
                .is_none_or(|value| value.trim().is_empty())
        {
            self.show_toast(ToastSeverity::Error, "Selected provider has no base_url");
            return false;
        }

        true
    }
}

fn normalize_pasted_text(text: &str) -> String {
    let sanitized_text = sanitize_terminal_text(text);
    let mut normalized = String::with_capacity(sanitized_text.len());
    let mut chars = sanitized_text.chars().peekable();

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum ComposerSelectionEdit {
    Replace(String),
    Delete,
    Kill,
    Yank,
}

fn composer_selection_edit_for_key(key: KeyEvent) -> Option<ComposerSelectionEdit> {
    match key.code {
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(ComposerSelectionEdit::Replace(character.to_string()))
        }
        KeyCode::Char('y') if is_ctrl_only(key.modifiers) => Some(ComposerSelectionEdit::Yank),
        KeyCode::Backspace | KeyCode::Delete if has_word_modifier(key.modifiers) => {
            Some(ComposerSelectionEdit::Kill)
        }
        KeyCode::Char('w') | KeyCode::Char('u') | KeyCode::Char('k')
            if is_ctrl_only(key.modifiers) =>
        {
            Some(ComposerSelectionEdit::Kill)
        }
        KeyCode::Char('d') if key.modifiers == KeyModifiers::ALT => {
            Some(ComposerSelectionEdit::Kill)
        }
        KeyCode::Char('h') | KeyCode::Char('d') if is_composer_selection_delete_key(key) => {
            Some(ComposerSelectionEdit::Delete)
        }
        KeyCode::Backspace | KeyCode::Delete => Some(ComposerSelectionEdit::Delete),
        _ => None,
    }
}

fn composer_navigation_key_clears_selection(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('a')
        | KeyCode::Char('b')
        | KeyCode::Char('e')
        | KeyCode::Char('f')
        | KeyCode::Char('n')
        | KeyCode::Char('p')
            if is_ctrl_only(key.modifiers) =>
        {
            true
        }
        KeyCode::Char('b') | KeyCode::Char('f') if key.modifiers == KeyModifiers::ALT => true,
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => true,
        KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown => true,
        _ => false,
    }
}

fn is_composer_selection_delete_key(key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('h') | KeyCode::Char('d') => {
            key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
        }
        _ => false,
    }
}

fn is_ctrl_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

fn has_word_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT)
}
