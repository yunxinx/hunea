//! TUI model 状态与子域装配。

use std::rc::Rc;
use std::time::Instant;

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
};
#[cfg(test)]
pub(super) use runtime_domain::session::{RuntimeToolActivity, RuntimeToolActivityUpdate};
use runtime_domain::{
    envinfo,
    model_catalog::{ModelCatalog, ModelSelection},
    prompt_assembly::{
        PromptAssemblyCandidateInventorySnapshot, PromptAssemblyCoreSystemSnapshot,
        PromptAssemblyInput, PromptAssemblyManagerSnapshot, PromptAssemblyResolvedSnapshot,
        PromptAssemblySourceInventorySnapshot, PromptPreludeSnapshot, resolve_prompt_assembly,
    },
    session::{PromptAssemblyUpdateNotice, RuntimeTerminalSnapshot, SessionLoadRequestId},
};

use super::{
    ReasoningDisplayMode, StartupBannerOptions,
    composer::{Composer, PendingComposerCursorClick},
    context_budget::ContextBudgetState,
    copy_picker::CopyPickerState,
    custom_prompt_picker::CustomPromptPickerState,
    entry_tree::{BRANCH_PICKER_LIST_ROWS_MAX, BRANCH_PICKER_LIST_ROWS_MIN},
    external_editor::ExternalEditorLaunch,
    file_picker::{FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT, FilePickerState},
    file_search::FileSearchCache,
    message_history_recall::BlindRecallState,
    message_revisit::MessageRevisitState,
    model_panel::ModelPanelState,
    render_frame::RenderFrame,
    selection::project_wide_selection_styles,
    skill_picker::SkillPickerState,
    startup_banner::StartupBannerEntranceState,
    status_line::StatusLineItem,
    status_phrases::StatusPhraseSelector,
    stream_activity::StreamActivityState,
    style_mode::StyleMode,
    theme::{TerminalPalette, default_palette},
    toast::ToastState,
    tool_approval_panel::ToolApprovalPanelState,
    transcript::{RenderResult, Transcript, index_only_render_result},
    view,
};

mod layout_sync;
mod metrics;
mod options;
mod runtime_events;
mod runtime_response;
mod state;

pub use metrics::RequestMetrics;
pub use options::{EscRewindMode, ModelOptions};
use runtime_response::{RuntimeResponseBuffer, StreamedRuntimeReasoning};
pub(crate) use state::PendingReasoningToggleClick;
use state::{DocumentRuntimeState, NoticeState, SelectionRuntimeState};

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(super) startup_banner_options: StartupBannerOptions,
    pub(super) startup_banner_entrance: StartupBannerEntranceState,
    pub(super) style_mode: StyleMode,
    pub(super) status_line_items: Vec<StatusLineItem>,
    pub(super) status_line_2_items: Vec<StatusLineItem>,
    pub(super) external_editor: Vec<String>,
    pub(super) external_editor_hint: String,
    pub(super) external_editor_helper_enabled: bool,
    pub(super) model_catalog: ModelCatalog,
    pub(super) selected_model: Option<ModelSelection>,
    pub(super) requires_model_selection: bool,
    pub(super) model_panel: ModelPanelState,
    pub(super) tool_approval_panel: ToolApprovalPanelState,
    pub(super) tool_approval_panel_revision: usize,
    pub(super) transcript_overlay: Option<crate::transcript_overlay::TranscriptOverlayState>,
    pub(super) session_picker: Option<crate::session_picker::SessionPickerState>,
    pub(super) session_preview: Option<crate::session_preview::SessionPreviewState>,
    pub(super) entry_tree: Option<crate::entry_tree::EntryTreeState>,
    pub(super) context_budget: Option<ContextBudgetState>,
    pub(super) pending_context_budget_cancellation: bool,
    pub(super) pending_prompt_assembly_commit: bool,
    pub(super) copy_picker: Option<CopyPickerState>,
    pub(super) message_history_picker:
        Option<crate::message_history_picker::MessageHistoryPickerState>,
    pub(super) prompt_assembly: PromptAssemblyManagerSnapshot,
    pub(super) prompt_overlay: Option<crate::prompt_overlay::PromptOverlayState>,
    pub(super) next_session_load_request_id: u64,
    pub(super) message_revisit: MessageRevisitState,
    pub(super) runtime_terminal_snapshots: Vec<RuntimeTerminalSnapshot>,
    pub(super) stream_activity: Option<StreamActivityState>,
    pub(super) runtime_response_buffer: RuntimeResponseBuffer,
    pub(super) streamed_runtime_reasoning: StreamedRuntimeReasoning,
    pub(super) runtime_turn_tool_call_count: usize,
    pub(super) runtime_final_body_divider_pending: bool,
    pub(super) runtime_final_body_divider_inserted: bool,
    pub(super) status_phrase_selector: StatusPhraseSelector,
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) file_picker: Option<FilePickerState>,
    pub(super) skill_picker: Option<SkillPickerState>,
    pub(super) custom_prompt_picker: Option<CustomPromptPickerState>,
    pub(super) file_search_cache: FileSearchCache,
    pub(super) dismissed_file_picker_token: Option<String>,
    pub(super) dismissed_skill_picker_token: Option<String>,
    pub(super) dismissed_custom_prompt_picker_token: Option<String>,
    pub(super) copy_on_mouse_selection_release: bool,
    pub(super) swap_enter_and_send: bool,
    pub(super) ctrl_c_clears_input: bool,
    pub(super) message_history_limit: usize,
    pub(super) blind_recall: BlindRecallState,
    pub(super) esc_interrupt_presses: u8,
    pub(super) esc_rewind_mode: EscRewindMode,
    pub(super) show_esc_interrupt_hint: bool,
    pub(super) file_picker_popup_height: u16,
    pub(super) branch_picker_list_rows: u16,
    pub(super) show_reasoning_content: bool,
    pub(super) reasoning_display_mode: ReasoningDisplayMode,
    pub(super) debug_commands_enabled: bool,
    pub(super) chat_interrupt_esc_count: u8,
    pub(super) selection_runtime: SelectionRuntimeState,
    pub(super) pending_composer_cursor_click: PendingComposerCursorClick,
    pub(super) pending_reasoning_toggle_click: PendingReasoningToggleClick,
    pub(super) git_branch: String,
    pub(super) current_dir: String,
    pub(super) last_request_metrics: Option<RequestMetrics>,
    pub(super) palette: TerminalPalette,
    pub(super) palette_version: usize,
    pub(super) transcript: Transcript,
    pub(super) transcript_render: Rc<RenderResult>,
    pub(super) transcript_render_version: usize,
    pub(super) composer: Composer,
    pub(super) width: u16,
    pub(super) height: u16,
    pub(super) document_runtime: DocumentRuntimeState,
    pub(super) has_palette: bool,
    pub(super) has_window: bool,
    pub(super) has_dark_background: bool,
    pub(super) notice_state: NoticeState,
    pub(super) toast_state: ToastState,
    pub(super) pending_prompt_assembly_notice: Option<PromptAssemblyUpdateNotice>,
    pub(super) status_line_revision: usize,
    quitting: bool,
}

impl Model {
    /// `new` 创建并初始化 TUI 模型。
    pub fn new(startup_banner_options: StartupBannerOptions) -> Self {
        Self::new_with_options(startup_banner_options, ModelOptions::default())
    }

    /// `new_with_style_mode` 创建并初始化带指定样式模式的 TUI 模型。
    pub fn new_with_style_mode(
        startup_banner_options: StartupBannerOptions,
        style_mode: StyleMode,
    ) -> Self {
        Self::new_with_options(
            startup_banner_options,
            ModelOptions {
                style_mode,
                ..ModelOptions::default()
            },
        )
    }

    /// `new_with_options` 创建并初始化带显式选项的 TUI 模型。
    pub fn new_with_options(
        mut startup_banner_options: StartupBannerOptions,
        options: ModelOptions,
    ) -> Self {
        let palette = default_palette();
        let style_mode = options.style_mode.normalized();
        let status_line_items = options.status_line_items;
        let status_line_2_items = options.status_line_2_items;
        let selected_model = options.selected_model;
        if startup_banner_options.model_name.is_none() {
            startup_banner_options.model_name = selected_model
                .as_ref()
                .map(|selection| selection.model_id.clone());
        }
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.append_startup_banner(startup_banner_options.clone());
        let transcript_render = Rc::new(index_only_render_result(
            transcript.progressive_item_metrics_index(),
        ));
        let git_branch = resolve_initial_git_branch(&status_line_items, &status_line_2_items);
        let current_dir = resolve_initial_current_dir(&status_line_items, &status_line_2_items);
        let prompt_assembly =
            options
                .prompt_assembly
                .unwrap_or_else(|| PromptAssemblyManagerSnapshot {
                    resolution: PromptAssemblyResolvedSnapshot {
                        assembly: resolve_prompt_assembly(&PromptAssemblyInput::default()),
                        prelude: PromptPreludeSnapshot::default(),
                    },
                    sources: PromptAssemblySourceInventorySnapshot {
                        managed: Vec::new(),
                        preview: Vec::new(),
                    },
                    candidates: PromptAssemblyCandidateInventorySnapshot {
                        extra_prompts: Vec::new(),
                        discovered_skills: Vec::new(),
                        manual_skills: Vec::new(),
                        tools: Vec::new(),
                        dynamic_environment: Vec::new(),
                    },
                    dynamic_environment_observations: Vec::new(),
                    core_system: PromptAssemblyCoreSystemSnapshot {
                        builtin_body: String::new(),
                        global_override: None,
                        project_override: None,
                    },
                    diagnostics: Vec::new(),
                });

        Self {
            startup_banner_options,
            startup_banner_entrance: StartupBannerEntranceState::default(),
            style_mode,
            status_line_items: status_line_items.clone(),
            status_line_2_items,
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            model_catalog: options.model_catalog,
            selected_model,
            requires_model_selection: options.requires_model_selection,
            model_panel: ModelPanelState::default(),
            tool_approval_panel: ToolApprovalPanelState::default(),
            tool_approval_panel_revision: 1,
            transcript_overlay: None,
            session_picker: None,
            session_preview: None,
            entry_tree: None,
            context_budget: None,
            pending_context_budget_cancellation: false,
            pending_prompt_assembly_commit: false,
            copy_picker: None,
            message_history_picker: None,
            prompt_assembly,
            prompt_overlay: None,
            next_session_load_request_id: 1,
            message_revisit: MessageRevisitState::default(),
            runtime_terminal_snapshots: Vec::new(),
            stream_activity: None,
            runtime_response_buffer: RuntimeResponseBuffer::default(),
            streamed_runtime_reasoning: StreamedRuntimeReasoning {
                item_indices: Vec::new(),
                displayed_content: String::new(),
            },
            runtime_turn_tool_call_count: 0,
            runtime_final_body_divider_pending: false,
            runtime_final_body_divider_inserted: false,
            status_phrase_selector: StatusPhraseSelector::new(
                options.status_phrases,
                options.status_phrase_order,
            ),
            command_panel_selected: 0,
            command_panel_scroll: 0,
            file_picker: None,
            skill_picker: None,
            custom_prompt_picker: None,
            file_search_cache: FileSearchCache::default(),
            dismissed_file_picker_token: None,
            dismissed_skill_picker_token: None,
            dismissed_custom_prompt_picker_token: None,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            message_history_limit: options.message_history_limit,
            blind_recall: BlindRecallState::default(),
            esc_interrupt_presses: options.esc_interrupt_presses.clamp(1, 3),
            esc_rewind_mode: options.esc_rewind_mode,
            show_esc_interrupt_hint: options.show_esc_interrupt_hint,
            file_picker_popup_height: options
                .file_picker_popup_height
                .clamp(FILE_PICKER_POPUP_MIN_HEIGHT, FILE_PICKER_POPUP_MAX_HEIGHT),
            branch_picker_list_rows: options
                .branch_picker_list_rows
                .clamp(BRANCH_PICKER_LIST_ROWS_MIN, BRANCH_PICKER_LIST_ROWS_MAX),
            show_reasoning_content: options.show_reasoning_content,
            reasoning_display_mode: options.reasoning_display_mode,
            debug_commands_enabled: options.debug_commands_enabled,
            chat_interrupt_esc_count: 0,
            selection_runtime: SelectionRuntimeState::default(),
            pending_composer_cursor_click: PendingComposerCursorClick::default(),
            pending_reasoning_toggle_click: PendingReasoningToggleClick::default(),
            git_branch,
            current_dir,
            last_request_metrics: None,
            palette,
            palette_version: 1,
            transcript_render_version: 1,
            transcript,
            transcript_render,
            composer: Composer::new_with_undo_limit(style_mode, options.composer_undo_limit),
            width: 0,
            height: 0,
            document_runtime: DocumentRuntimeState {
                follow_bottom: true,
                ..DocumentRuntimeState::default()
            },
            has_palette: false,
            has_window: false,
            has_dark_background: true,
            notice_state: NoticeState::default(),
            toast_state: ToastState::default(),
            pending_prompt_assembly_notice: None,
            status_line_revision: 1,
            quitting: false,
        }
    }

    /// `render_to_buffer` 将当前模型渲染到指定屏幕缓冲区。
    #[must_use = "`render_to_buffer` 返回渲染后的 cursor 位置，调用方必须传递或显式丢弃"]
    pub fn render_to_buffer(&mut self, area: Rect, buffer: &mut Buffer) -> Option<Position> {
        self.render_to_buffer_at(Instant::now(), area, buffer)
    }

    pub(crate) fn render_to_buffer_at(
        &mut self,
        now: Instant,
        area: Rect,
        buffer: &mut Buffer,
    ) -> Option<Position> {
        let mut frame = RenderFrame::new_at(now, area, buffer);
        view::render(self, &mut frame);
        project_wide_selection_styles(frame.buffer_mut());
        frame.cursor_position()
    }

    /// `palette` 返回当前使用的配色。
    pub fn palette(&self) -> &TerminalPalette {
        &self.palette
    }

    /// `has_palette` 返回是否已经拿到可用配色。
    pub fn has_palette(&self) -> bool {
        self.has_palette
    }

    /// `is_ready` 判断首帧是否具备稳定布局和主题信息。
    pub fn is_ready(&self) -> bool {
        self.has_palette && self.has_window
    }

    /// `is_quitting` 返回是否正在退出。
    pub fn is_quitting(&self) -> bool {
        self.quitting
    }

    pub(super) fn model_selection_display_name(&self, provider_id: &str, model_id: &str) -> String {
        let provider_name = self
            .model_catalog
            .enabled_provider_by_id(provider_id)
            .map(|provider| provider.display_name.as_str())
            .unwrap_or(provider_id);
        format!("[{provider_name}] {model_id}")
    }

    /// `last_request_metrics` 返回最近一次成功完成请求的状态行指标。
    pub fn last_request_metrics(&self) -> Option<RequestMetrics> {
        self.last_request_metrics
    }

    /// `set_last_request_metrics` 更新最近一次成功完成请求的状态行指标。
    pub fn set_last_request_metrics(&mut self, metrics: Option<RequestMetrics>) {
        if self.last_request_metrics == metrics {
            return;
        }

        self.last_request_metrics = metrics;
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    /// `next_timeout_deadline` 返回当前最早需要处理的内部超时。
    pub fn next_timeout_deadline(&self) -> Option<Instant> {
        [
            self.notice_state.status_deadline,
            self.notice_state.external_editor_helper_deadline,
            self.notice_state.history_scroll_indicator_deadline,
            self.selection_runtime.auto_scroll_deadline,
            self.toast_timeout_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// `composer_text` 返回输入框当前的内容。
    pub fn composer_text(&self) -> &str {
        self.composer.value()
    }

    /// `transcript_plain_items` 返回适用于纯文本消费的 transcript 项列表。
    pub fn transcript_plain_items(&self) -> Vec<String> {
        self.transcript.plain_items()
    }

    /// `terminal_replay_items` 返回退出 AltScreen 后回放到终端的 transcript 项。
    pub fn terminal_replay_items(&self, preserve_ansi: bool) -> Vec<String> {
        self.transcript.terminal_replay_items(preserve_ansi)
    }

    pub(crate) fn set_window(&mut self, width: u16, height: u16) {
        let width = width.max(1);
        let width_changed = !self.has_window || self.width != width;

        self.width = width;
        self.height = height;
        self.has_window = true;
        self.transcript.set_width(width);
        if let Some(preview) = self.session_preview.as_mut() {
            preview.transcript.set_width(width);
        }
        self.sync_copy_picker_preview_width(width);
        self.sync_entry_tree_preview_width(width);
        self.sync_message_history_picker_preview_width(width);
        self.sync_prompt_overlay_preview_width(width);
        self.composer.set_width(width);
        if width_changed {
            self.sync_transcript_render();
        }
        self.sync_tool_approval_preview_mode();
        self.sync_command_panel_navigation();
        self.sync_composer_attached_picker_state();
        self.sync_composer_height();
    }

    pub(crate) fn set_palette(&mut self, palette: TerminalPalette, has_dark_background: bool) {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let palette_changed = self.palette != palette;
        if palette_changed && self.selection_runtime.selection.is_active() {
            self.invalidate_selection_for_reflow();
        }
        if palette_changed {
            self.palette_version += 1;
        }
        self.palette = palette;
        self.has_dark_background = has_dark_background;
        self.has_palette = true;
        self.transcript.set_palette(palette);
        if let Some(preview) = self.session_preview.as_mut() {
            preview.transcript.set_palette(palette);
        }
        self.sync_copy_picker_preview_palette(palette);
        self.sync_entry_tree_preview_palette(palette);
        if palette_changed {
            self.sync_transcript_render();
        }
        self.sync_composer_height();
        if palette_changed {
            self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        }
    }

    pub(crate) fn composer_mut(&mut self) -> &mut Composer {
        &mut self.composer
    }

    #[cfg(test)]
    #[allow(dead_code)] // 供 model/tests 断言；clippy lib-test 单元有时未关联到调用点
    pub(crate) fn blind_recall(&self) -> &BlindRecallState {
        &self.blind_recall
    }

    pub(crate) fn transcript_mut(&mut self) -> &mut Transcript {
        &mut self.transcript
    }

    pub(crate) fn reset_to_initial_tui_state(&mut self) {
        self.startup_banner_entrance.complete();
        let mut transcript = Transcript::new(self.palette);
        transcript.set_gap(1);
        if self.has_window {
            transcript.set_width(self.width);
        }
        transcript.append_startup_banner(self.startup_banner_options.clone());
        self.transcript = transcript;
        self.composer.clear();
        self.model_panel = ModelPanelState::default();
        self.session_picker = None;
        self.session_preview = None;
        self.entry_tree = None;
        self.copy_picker = None;
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.message_revisit = MessageRevisitState::default();
        self.runtime_terminal_snapshots.clear();
        self.stream_activity = None;
        self.runtime_response_buffer.clear();
        self.command_panel_selected = 0;
        self.command_panel_scroll = 0;
        self.file_picker = None;
        self.skill_picker = None;
        self.custom_prompt_picker = None;
        self.dismissed_file_picker_token = None;
        self.dismissed_skill_picker_token = None;
        self.dismissed_custom_prompt_picker_token = None;
        self.selection_runtime = SelectionRuntimeState::default();
        self.pending_composer_cursor_click = PendingComposerCursorClick::default();
        self.pending_reasoning_toggle_click = PendingReasoningToggleClick::default();
        self.set_last_request_metrics(None);
        self.document_runtime = DocumentRuntimeState {
            follow_bottom: true,
            ..DocumentRuntimeState::default()
        };
        self.notice_state = NoticeState::default();
        self.toast_state = ToastState::default();
        self.bump_status_line_revision();
        self.sync_transcript_render();
        self.sync_composer_height();
        self.sync_document_viewport_to_bottom();
    }

    pub(crate) fn startup_banner_entrance_frame_interval_at(
        &self,
        now: Instant,
    ) -> Option<std::time::Duration> {
        if !self.startup_banner_entrance_target_renderable() {
            return None;
        }

        self.startup_banner_entrance.frame_interval_at(now)
    }

    pub(crate) fn apply_startup_banner_entrance_at(
        &mut self,
        now: Instant,
        buffer: &mut ratatui::buffer::Buffer,
        area: ratatui::layout::Rect,
    ) {
        let slide_fill_color = self.palette.surface.unwrap_or_else(|| {
            startup_banner_entrance_fallback_fill_color(self.has_dark_background)
        });
        self.startup_banner_entrance
            .apply_at(now, buffer, area, slide_fill_color);
    }

    pub(crate) fn startup_banner_entrance_target_available(&self) -> bool {
        self.transcript.starts_with_startup_banner()
    }

    pub(crate) fn startup_banner_entrance_target_renderable(&self) -> bool {
        self.startup_banner_entrance_target_available()
            && !self.modal_obscures_startup_banner_entrance_target()
    }

    pub(crate) fn complete_startup_banner_entrance(&mut self) {
        self.startup_banner_entrance.complete();
    }

    #[cfg(test)]
    pub(crate) fn start_startup_banner_entrance_for_test(&mut self, now: Instant) {
        self.startup_banner_entrance.start_for_test(now);
    }

    #[cfg(test)]
    pub(crate) fn complete_startup_banner_entrance_for_test(&mut self) {
        self.startup_banner_entrance.complete();
    }

    #[cfg(test)]
    pub(crate) fn startup_banner_entrance_completed_for_test(&self) -> bool {
        self.startup_banner_entrance.is_completed()
    }

    pub(crate) fn mark_quitting(&mut self) {
        self.quitting = true;
    }

    pub(crate) fn timeout_event(&self, now: Instant) -> Option<super::AppEvent> {
        if let Some(deadline) = self.notice_state.status_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::StatusNoticeTimeout {
                token: self.notice_state.status_token,
            });
        }

        if let Some(deadline) = self.notice_state.history_scroll_indicator_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::HistoryScrollIndicatorTimeout {
                token: self.notice_state.history_scroll_indicator_token,
            });
        }

        if let Some(deadline) = self.notice_state.external_editor_helper_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::ExternalEditorHelperTimeout {
                token: self.notice_state.external_editor_helper_token,
            });
        }

        if let Some(deadline) = self.selection_runtime.auto_scroll_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::SelectionAutoScrollTick {
                token: self.selection_runtime.auto_scroll_token,
            });
        }

        if let Some(deadline) = self.toast_timeout_deadline()
            && now >= deadline
        {
            return Some(super::AppEvent::ToastNoticeTimeout {
                token: self.toast_timeout_token(),
            });
        }

        None
    }

    pub(crate) fn maybe_prepare_external_editor_launch(&mut self) -> Option<ExternalEditorLaunch> {
        self.prepare_external_editor_launch()
    }

    pub(crate) fn next_session_load_request_id(&mut self) -> SessionLoadRequestId {
        let request_id = SessionLoadRequestId::new(self.next_session_load_request_id);
        self.next_session_load_request_id = self.next_session_load_request_id.wrapping_add(1);
        if self.next_session_load_request_id == 0 {
            self.next_session_load_request_id = 1;
        }
        request_id
    }
}

fn startup_banner_entrance_fallback_fill_color(has_dark_background: bool) -> ratatui::style::Color {
    if has_dark_background {
        ratatui::style::Color::from_u32(0x1d2021)
    } else {
        ratatui::style::Color::from_u32(0xf2f0ee)
    }
}

fn resolve_initial_git_branch(
    status_line_items: &[StatusLineItem],
    status_line_2_items: &[StatusLineItem],
) -> String {
    if status_line_items
        .iter()
        .chain(status_line_2_items)
        .any(|item| matches!(item, StatusLineItem::GitBranch))
    {
        envinfo::git_branch()
    } else {
        String::new()
    }
}

fn resolve_initial_current_dir(
    status_line_items: &[StatusLineItem],
    status_line_2_items: &[StatusLineItem],
) -> String {
    if status_line_items
        .iter()
        .chain(status_line_2_items)
        .any(|item| matches!(item, StatusLineItem::CurrentDir))
    {
        envinfo::short_work_dir()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests;
