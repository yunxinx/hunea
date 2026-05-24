use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::Frame;
use runtime_domain::{
    envinfo,
    model_catalog::{ModelCatalog, ModelSelection},
    phrases::StatusPhraseOrder,
    session::{RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityUpdate},
};

use super::{
    ReasoningDisplayMode, Sender, StartupBannerOptions,
    composer::Composer,
    composer_mouse::PendingComposerCursorClick,
    document::offset_viewport_line_indices,
    external_editor::ExternalEditorLaunch,
    file_picker::{FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT, FilePickerState},
    file_search::FileSearchCache,
    message_revisit::MessageRevisitState,
    model_panel::ModelPanelState,
    status_line::{
        StatusLineItem, StatusLineRenderResult, status_line_gap_before, status_line_pair_height,
    },
    status_phrases::{StatusPhraseSelector, default_status_phrases},
    stream_activity::StreamActivityState,
    style_mode::StyleMode,
    theme::{TerminalPalette, default_palette},
    tool_approval_panel::ToolApprovalPanelState,
    transcript::{RenderResult, Transcript, TranscriptEstimateBreakdown, index_only_render_result},
    view,
};

mod runtime_response;
mod state;

use runtime_response::{
    BufferedRuntimeResponse, RuntimeResponseBuffer, StreamedRuntimeReasoning,
    strip_displayed_reasoning_prefix,
};
pub(crate) use state::PendingReasoningToggleClick;
use state::{DocumentRuntimeState, NoticeState, SelectionRuntimeState};

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(super) startup_banner_options: StartupBannerOptions,
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
    pub(super) message_revisit: MessageRevisitState,
    pub(super) runtime_terminal_snapshots: Vec<RuntimeTerminalSnapshot>,
    pub(super) stream_activity: Option<StreamActivityState>,
    pub(super) runtime_response_buffer: RuntimeResponseBuffer,
    pub(super) streamed_runtime_reasoning: StreamedRuntimeReasoning,
    pub(super) status_phrase_selector: StatusPhraseSelector,
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) file_picker: Option<FilePickerState>,
    pub(super) file_search_cache: FileSearchCache,
    pub(super) dismissed_file_picker_token: Option<String>,
    pub(super) copy_on_mouse_selection_release: bool,
    pub(super) swap_enter_and_send: bool,
    pub(super) ctrl_c_clears_input: bool,
    pub(super) esc_interrupt_presses: u8,
    pub(super) show_esc_interrupt_hint: bool,
    pub(super) file_picker_popup_height: u16,
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
    pub(super) status_line_revision: usize,
    quitting: bool,
}

/// `RequestMetrics` 保存最近一次成功完成请求的状态行指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

impl RequestMetrics {
    /// `new` 创建最近一次成功请求的性能指标。
    pub fn new(latency: Duration, output_tokens: usize, duration: Duration) -> Self {
        Self {
            latency,
            output_tokens,
            duration,
        }
    }
}

/// `ModelOptions` 表示创建 TUI 模型时可配置的样式与状态行选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOptions {
    pub style_mode: StyleMode,
    pub status_line_items: Vec<StatusLineItem>,
    pub status_line_2_items: Vec<StatusLineItem>,
    pub external_editor: Vec<String>,
    pub external_editor_hint: String,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub esc_interrupt_presses: u8,
    pub show_esc_interrupt_hint: bool,
    pub file_picker_popup_height: u16,
    pub show_reasoning_content: bool,
    pub reasoning_display_mode: ReasoningDisplayMode,
    pub debug_commands_enabled: bool,
    pub model_catalog: ModelCatalog,
    pub selected_model: Option<ModelSelection>,
    pub requires_model_selection: bool,
    pub status_phrases: Vec<String>,
    pub status_phrase_order: StatusPhraseOrder,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            style_mode: StyleMode::default(),
            status_line_items: Vec::new(),
            status_line_2_items: Vec::new(),
            external_editor: Vec::new(),
            external_editor_hint: String::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            esc_interrupt_presses: 2,
            show_esc_interrupt_hint: true,
            file_picker_popup_height: 7,
            show_reasoning_content: false,
            reasoning_display_mode: ReasoningDisplayMode::Collapsed,
            debug_commands_enabled: false,
            model_catalog: ModelCatalog::default(),
            selected_model: None,
            requires_model_selection: false,
            status_phrases: default_status_phrases(),
            status_phrase_order: StatusPhraseOrder::Random,
        }
    }
}

/// `TranscriptSyncProfile` 记录一次 transcript sync 在首帧前的关键耗时拆分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptSyncProfile {
    pub(crate) estimate_time: Duration,
    pub(crate) visible_exact_time: Duration,
    pub(crate) estimate_breakdown: TranscriptEstimateBreakdown,
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
        startup_banner_options: StartupBannerOptions,
        options: ModelOptions,
    ) -> Self {
        let palette = default_palette();
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.append_startup_banner(startup_banner_options.clone());
        let transcript_render = Rc::new(index_only_render_result(
            transcript.progressive_item_metrics_index(),
        ));
        let style_mode = options.style_mode.normalized();
        let status_line_items = options.status_line_items;
        let status_line_2_items = options.status_line_2_items;
        let selected_model = options
            .selected_model
            .filter(|selection| options.model_catalog.contains_selection(selection));
        let git_branch = resolve_initial_git_branch(&status_line_items, &status_line_2_items);
        let current_dir = resolve_initial_current_dir(&status_line_items, &status_line_2_items);

        Self {
            startup_banner_options,
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
            message_revisit: MessageRevisitState::default(),
            runtime_terminal_snapshots: Vec::new(),
            stream_activity: None,
            runtime_response_buffer: RuntimeResponseBuffer::default(),
            streamed_runtime_reasoning: StreamedRuntimeReasoning {
                item_indices: Vec::new(),
                displayed_content: String::new(),
            },
            status_phrase_selector: StatusPhraseSelector::new(
                options.status_phrases,
                options.status_phrase_order,
            ),
            command_panel_selected: 0,
            command_panel_scroll: 0,
            file_picker: None,
            file_search_cache: FileSearchCache::default(),
            dismissed_file_picker_token: None,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            esc_interrupt_presses: options.esc_interrupt_presses.clamp(1, 3),
            show_esc_interrupt_hint: options.show_esc_interrupt_hint,
            file_picker_popup_height: options
                .file_picker_popup_height
                .clamp(FILE_PICKER_POPUP_MIN_HEIGHT, FILE_PICKER_POPUP_MAX_HEIGHT),
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
            composer: Composer::new(style_mode),
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
            status_line_revision: 1,
            quitting: false,
        }
    }

    /// `render` 将当前模型渲染到一帧。
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        view::render(self, frame);
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
        self.composer.set_width(width);
        if width_changed {
            self.sync_transcript_render();
        }
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
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

    pub(crate) fn transcript_mut(&mut self) -> &mut Transcript {
        &mut self.transcript
    }

    pub(crate) fn reset_to_initial_tui_state(&mut self) {
        let mut transcript = Transcript::new(self.palette);
        transcript.set_gap(1);
        if self.has_window {
            transcript.set_width(self.width);
        }
        transcript.append_startup_banner(self.startup_banner_options.clone());
        self.transcript = transcript;
        self.composer.clear();
        self.model_panel = ModelPanelState::default();
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.message_revisit = MessageRevisitState::default();
        self.runtime_terminal_snapshots.clear();
        self.stream_activity = None;
        self.runtime_response_buffer.clear();
        self.command_panel_selected = 0;
        self.command_panel_scroll = 0;
        self.file_picker = None;
        self.dismissed_file_picker_token = None;
        self.selection_runtime = SelectionRuntimeState::default();
        self.pending_composer_cursor_click = PendingComposerCursorClick::default();
        self.pending_reasoning_toggle_click = PendingReasoningToggleClick::default();
        self.set_last_request_metrics(None);
        self.document_runtime = DocumentRuntimeState {
            follow_bottom: true,
            ..DocumentRuntimeState::default()
        };
        self.notice_state = NoticeState::default();
        self.bump_status_line_revision();
        self.sync_transcript_render();
        self.sync_composer_height();
        self.sync_document_viewport_to_bottom();
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

        None
    }

    pub(crate) fn maybe_prepare_external_editor_launch(&mut self) -> Option<ExternalEditorLaunch> {
        self.prepare_external_editor_launch()
    }

    pub(crate) fn append_assistant_message_from_runtime(&mut self, content: impl Into<String>) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        self.transcript_mut().append_message_with_style_mode(
            Sender::Assistant,
            content,
            style_mode,
        );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_runtime_response_from_runtime(
        &mut self,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        reasoning_duration: Option<std::time::Duration>,
    ) {
        let content = content.into();
        let reasoning_content = reasoning_content
            .filter(|content| !content.trim().is_empty())
            .filter(|_| self.show_reasoning_content);

        if content.is_empty() && reasoning_content.is_none() {
            return;
        }

        if let Some(reasoning_content) = reasoning_content {
            self.append_assistant_message_with_reasoning_from_runtime(
                content,
                reasoning_content,
                reasoning_duration,
            );
            return;
        }

        self.append_assistant_message_from_runtime(content);
    }

    pub(crate) fn push_runtime_assistant_delta(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        if self.runtime_response_buffer.is_empty() {
            let _ = self.mark_exploration_tool_activities_complete_from_runtime();
        }
        self.flush_runtime_reasoning_for_expanded_display();
        self.runtime_response_buffer.push_content(content);
    }

    pub(crate) fn push_runtime_reasoning_delta(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        if self.runtime_response_buffer.is_empty() {
            let _ = self.mark_exploration_tool_activities_complete_from_runtime();
        }
        self.runtime_response_buffer.push_reasoning_content(content);
    }

    pub(crate) fn flush_runtime_response_buffer(&mut self) {
        if let Some(response) = self.runtime_response_buffer.take() {
            self.append_buffered_runtime_response_from_runtime(
                response,
                self.streams_reasoning_into_transcript_during_response(),
            );
        }
    }

    pub(crate) fn flush_runtime_response_buffer_with_final(
        &mut self,
        final_content: String,
        final_reasoning_content: Option<String>,
        final_reasoning_duration: Option<Duration>,
    ) {
        // Expanded reasoning that already crossed a text/tool boundary is
        // committed display state. The final provider `reasoning_content`
        // should only supplement the still-buffered tail that has not been
        // rendered yet.
        let displayed_reasoning_content = self.streamed_runtime_reasoning.displayed_content.clone();
        let buffered_response = if displayed_reasoning_content.is_empty() {
            self.runtime_response_buffer.take_with_final(
                final_content,
                final_reasoning_content,
                final_reasoning_duration,
            )
        } else if self.runtime_response_buffer.has_reasoning_content() {
            self.runtime_response_buffer.take_with_final(
                final_content,
                strip_displayed_reasoning_prefix(
                    final_reasoning_content,
                    &displayed_reasoning_content,
                ),
                final_reasoning_duration,
            )
        } else {
            self.runtime_response_buffer
                .take_with_final(final_content, None, None)
        };

        if let Some(response) = buffered_response {
            self.append_runtime_response_from_runtime(
                response.content,
                response.reasoning_content,
                response.reasoning_duration,
            );
        }
        self.accept_streamed_runtime_reasoning_from_runtime();
    }

    pub(crate) fn clear_runtime_response_buffer(&mut self) {
        self.runtime_response_buffer.clear();
        self.discard_streamed_runtime_reasoning_from_runtime();
    }

    pub(crate) fn streams_reasoning_into_transcript_during_response(&self) -> bool {
        self.show_reasoning_content
            && matches!(self.reasoning_display_mode, ReasoningDisplayMode::Expanded)
    }

    pub(crate) fn flush_runtime_reasoning_for_expanded_display(&mut self) {
        if !self.streams_reasoning_into_transcript_during_response() {
            return;
        }

        let Some(reasoning) = self
            .runtime_response_buffer
            .take_reasoning_for_expanded_display()
        else {
            return;
        };
        self.append_buffered_runtime_response_from_runtime(
            BufferedRuntimeResponse {
                content: String::new(),
                reasoning_content: Some(reasoning.content),
                reasoning_duration: reasoning.duration,
            },
            true,
        );
    }

    pub(crate) fn accept_streamed_runtime_reasoning_from_runtime(&mut self) {
        self.streamed_runtime_reasoning.item_indices.clear();
        self.streamed_runtime_reasoning.displayed_content.clear();
    }

    fn append_buffered_runtime_response_from_runtime(
        &mut self,
        response: BufferedRuntimeResponse,
        track_streamed_reasoning: bool,
    ) {
        let tracked_reasoning_content = track_streamed_reasoning
            .then_some(response.reasoning_content.as_deref())
            .flatten()
            .map(str::to_owned);
        let reasoning_item_index = track_streamed_reasoning
            .then_some(response.reasoning_content.as_ref())
            .flatten()
            .map(|_| self.transcript.len());

        self.append_runtime_response_from_runtime(
            response.content,
            response.reasoning_content,
            response.reasoning_duration,
        );

        if let Some(item_index) = reasoning_item_index
            && self.transcript.len() > item_index
        {
            self.streamed_runtime_reasoning
                .item_indices
                .push(item_index);
        }

        if let Some(reasoning_content) = tracked_reasoning_content {
            self.streamed_runtime_reasoning
                .displayed_content
                .push_str(&reasoning_content);
        }
    }

    fn discard_streamed_runtime_reasoning_from_runtime(&mut self) {
        let item_indices = std::mem::take(&mut self.streamed_runtime_reasoning.item_indices);
        if item_indices.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self.transcript_mut().remove_items(&item_indices) {
            return;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    fn append_assistant_message_with_reasoning_from_runtime(
        &mut self,
        content: impl Into<String>,
        reasoning_content: impl Into<String>,
        reasoning_duration: Option<std::time::Duration>,
    ) {
        let content = content.into();
        let reasoning_content = reasoning_content.into();
        if content.is_empty() && reasoning_content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let style_mode = self.style_mode;
        let reasoning_display_mode = self.reasoning_display_mode;
        self.transcript_mut()
            .append_assistant_message_with_reasoning(
                content,
                reasoning_content,
                reasoning_display_mode,
                reasoning_duration,
                style_mode,
            );
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn toggle_reasoning_item(&mut self, item_index: usize) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self.transcript_mut().toggle_reasoning_item(item_index) {
            return false;
        }

        self.sync_transcript_render();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn append_system_message_from_runtime(&mut self, content: impl Into<String>) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_system_message(content);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_work_duration_from_runtime(&mut self, duration: Duration) {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_work_duration_message(duration);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_tool_result_from_runtime(
        &mut self,
        content: impl Into<String>,
        kind: super::tool_result::ToolResultKind,
    ) {
        let content = content.into();
        if content.is_empty() {
            return;
        }

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript_mut().append_tool_result(content, kind);
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(crate) fn append_runtime_tool_activity_from_runtime(
        &mut self,
        call: impl Into<RuntimeToolActivity>,
    ) -> usize {
        let call = call.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        let item_index = self.transcript_mut().append_runtime_tool_activity(call);
        let snapshots = self.runtime_terminal_snapshots.clone();
        for snapshot in snapshots {
            let _ = self
                .transcript_mut()
                .set_runtime_terminal_snapshot(snapshot);
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        item_index
    }

    pub(crate) fn mark_exploration_tool_activities_complete_from_runtime(&mut self) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .mark_exploration_tool_activities_complete()
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn runtime_tool_activity_item_index_from_runtime(
        &self,
        tool_call_id: &str,
    ) -> Option<usize> {
        self.transcript.runtime_tool_activity_index(tool_call_id)
    }

    pub(crate) fn update_runtime_tool_activity_from_runtime(
        &mut self,
        item_index: usize,
        update: impl Into<RuntimeToolActivityUpdate>,
    ) -> bool {
        let update = update.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .update_runtime_tool_activity(item_index, update)
        {
            return false;
        }
        let snapshots = self.runtime_terminal_snapshots.clone();
        for snapshot in snapshots {
            let _ = self
                .transcript_mut()
                .set_runtime_terminal_snapshot(snapshot);
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        if preserved_viewport_state.is_none() {
            self.document_runtime.follow_bottom = true;
        }
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    /// `suspend_runtime_tool_activity_approval_from_runtime` 在审批面板打开期间隐藏重复的等待行。
    pub(crate) fn suspend_runtime_tool_activity_approval_from_runtime(
        &mut self,
        activity_id: &str,
    ) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .set_runtime_tool_activity_approval_suspended(activity_id, true)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    /// `clear_runtime_tool_activity_approval_suspensions_from_runtime` 恢复被审批面板隐藏的工具行。
    pub(crate) fn clear_runtime_tool_activity_approval_suspensions_from_runtime(&mut self) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .clear_runtime_tool_activity_approval_suspensions()
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn apply_runtime_terminal_snapshot_from_runtime(
        &mut self,
        snapshot: impl Into<RuntimeTerminalSnapshot>,
    ) -> bool {
        let snapshot = snapshot.into();
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.runtime_terminal_snapshots
            .retain(|stored| stored.terminal_id != snapshot.terminal_id);
        self.runtime_terminal_snapshots.push(snapshot.clone());
        if !self
            .transcript_mut()
            .set_runtime_terminal_snapshot(snapshot)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        if preserved_viewport_state.is_none() {
            self.document_runtime.follow_bottom = true;
        }
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn sync_composer_height(&mut self) {
        let full_height = self.composer.full_height().max(1);
        let mut viewport_height = if !self.has_window || self.height == 0 {
            full_height
        } else {
            full_height.min(self.height.max(1))
        };

        let status_line = self.current_status_line_render_result();
        let status_line_2 = self.current_status_line_2_render_result();
        let command_panel = self.current_inline_command_panel_render_result();
        let model_panel = self.current_inline_model_panel_render_result();
        let tool_approval_panel = self.current_inline_tool_approval_panel_render_result();
        if status_line.has_content
            || status_line_2.has_content
            || command_panel.has_content
            || model_panel.has_content
            || tool_approval_panel.has_content
        {
            if self.document_runtime.follow_bottom && !self.document_runtime.manual_scroll {
                let panel_rows = command_panel.lines.len()
                    + model_panel.lines.len()
                    + tool_approval_panel.lines.len();
                let visible_height = self.bottom_follow_composer_content_line_count(
                    &status_line,
                    &status_line_2,
                    panel_rows,
                );
                viewport_height =
                    viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
            } else {
                let visible_height = self.visible_composer_content_line_count_in_viewport();
                if visible_height > 0 {
                    viewport_height =
                        viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
                }
            }
        }

        self.composer.set_height(viewport_height);
    }

    /// `sync_transcript_render` 只刷新 transcript 的 metrics/index 摘要，
    /// 不在 sync 阶段做全文 block materialization。
    pub(crate) fn sync_transcript_render(&mut self) {
        let _ = self.sync_transcript_render_profile_impl(false);
    }

    pub(crate) fn sync_transcript_render_profile(&mut self) -> TranscriptSyncProfile {
        self.sync_transcript_render_profile_impl(true)
    }

    fn sync_transcript_render_profile_impl(
        &mut self,
        collect_breakdown: bool,
    ) -> TranscriptSyncProfile {
        let previous_overlay_index = self
            .transcript_overlay
            .as_ref()
            .map(|_| self.transcript_render.index.clone());

        // metrics-only rebuild 不应保留旧 viewport 预热留下的 render block。
        self.transcript.begin_recent_render_block_batch();
        let estimate_started_at = Instant::now();
        let (index, estimate_breakdown) = if collect_breakdown {
            self.transcript
                .progressive_item_metrics_index_with_breakdown()
        } else {
            (
                self.transcript.progressive_item_metrics_index(),
                TranscriptEstimateBreakdown::default(),
            )
        };
        let estimate_time = estimate_started_at.elapsed();
        self.transcript.finish_recent_render_block_batch(0);
        let visible_exact_started_at = Instant::now();
        let index = self.exactize_visible_transcript_window_until_stable(index);
        let visible_exact_time = visible_exact_started_at.elapsed();
        let next_overlay_index = index.clone();
        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.invalidate_document_viewport_cache();
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        if let Some(previous_overlay_index) = previous_overlay_index.as_ref() {
            self.sync_transcript_overlay_after_transcript_refresh(
                previous_overlay_index,
                &next_overlay_index,
            );
        }
        TranscriptSyncProfile {
            estimate_time,
            visible_exact_time,
            estimate_breakdown,
        }
    }

    pub(crate) fn ensure_current_transcript_window_exact(&mut self) {
        // render 阶段 exactization 发生在 layout 构建内部，不能再递归抓当前 layout；
        // 这里直接复用现有 viewport 状态作为手动滚动恢复锚点。
        let preserved_viewport_state = self
            .document_runtime
            .manual_scroll
            .then(|| self.document_runtime.viewport_state.clone());
        let index = self
            .exactize_visible_transcript_window_until_stable(self.transcript_render.index.clone());
        if index == self.transcript_render.index {
            return;
        }

        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        self.document_runtime.viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    fn exactize_visible_transcript_window_until_stable(
        &mut self,
        mut index: crate::transcript::TranscriptItemMetricsIndex,
    ) -> crate::transcript::TranscriptItemMetricsIndex {
        let mut remaining_items = index.metrics.len();
        while remaining_items > 0 {
            let Some((start, count)) = self.current_visible_transcript_window_for_index(&index)
            else {
                break;
            };
            let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                break;
            }

            drop(index);
            self.release_transcript_index_holders_for_exactization();
            let Some((start_item, end_item)) =
                self.transcript
                    .exactize_line_window(start, count, overscan_lines)
            else {
                index = self.transcript.progressive_item_metrics_index();
                break;
            };
            let next_index = self.transcript.progressive_item_metrics_index();
            index = next_index;
            remaining_items = remaining_items.saturating_sub(end_item.saturating_sub(start_item));
        }

        index
    }

    fn release_transcript_index_holders_for_exactization(&mut self) {
        self.transcript_render = Rc::new(index_only_render_result(
            crate::transcript::TranscriptItemMetricsIndex::default(),
        ));
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        self.document_runtime.viewport_cache = Default::default();
    }

    pub(crate) fn status_line_revision(&self) -> usize {
        self.status_line_revision
    }

    pub(crate) fn bump_status_line_revision(&mut self) {
        self.status_line_revision = self.status_line_revision.saturating_add(1);
    }

    fn bottom_follow_composer_content_line_count(
        &self,
        status_line: &StatusLineRenderResult,
        status_line_2: &StatusLineRenderResult,
        panel_rows: usize,
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let stream_activity = self.current_stream_activity_render_result();
        let mut tail_rows = panel_rows;
        if stream_activity.has_content {
            tail_rows += 1;
        }
        tail_rows += status_line_pair_height(
            status_line,
            status_line_2,
            status_line_gap_before(self.style_mode),
        );
        if self.composer_uses_rendered_frame_padding() {
            tail_rows += 1;
        }

        if tail_rows < viewport_height {
            viewport_height - tail_rows
        } else {
            viewport_height
        }
    }

    pub(crate) fn composer_uses_rendered_frame_padding(&self) -> bool {
        match self.style_mode {
            StyleMode::Cx => self.palette.surface.is_some(),
            StyleMode::Cc => true,
            StyleMode::Ms => false,
        }
    }

    fn visible_composer_content_line_count_in_viewport(&mut self) -> usize {
        let layout = self.build_document_layout();
        let line_indices = offset_viewport_line_indices(
            &layout,
            self.document_runtime.viewport_y,
            self.document_viewport_height(),
        );

        line_indices
            .into_iter()
            .filter(|line_index| {
                *line_index >= layout.composer_slot.content_start_line
                    && *line_index <= layout.composer_slot.content_bottom_line()
            })
            .count()
    }

    /// `current_visible_transcript_window` 返回当前 document viewport 与 transcript 的交集窗口。
    pub(crate) fn current_visible_transcript_window(
        &mut self,
        transcript_line_count: usize,
    ) -> Option<(usize, usize)> {
        if transcript_line_count == 0 || self.document_viewport_height() == 0 {
            return None;
        }

        if self.document_runtime.viewport_state.manual_scroll() {
            let index = self.transcript.progressive_item_metrics_index();
            return self.current_visible_transcript_window_for_index(&index);
        }

        let layout = self.transcript_window_layout(transcript_line_count);
        self.current_visible_transcript_window_for_layout(&layout, transcript_line_count, false)
    }

    fn current_visible_transcript_window_for_index(
        &mut self,
        index: &crate::transcript::TranscriptItemMetricsIndex,
    ) -> Option<(usize, usize)> {
        if index.line_count == 0 || self.document_viewport_height() == 0 {
            return None;
        }

        let manual_scroll = self.document_runtime.viewport_state.manual_scroll();
        let layout = self.document_layout_for_transcript_index(index.clone());
        self.current_visible_transcript_window_for_layout(&layout, index.line_count, manual_scroll)
    }

    fn current_visible_transcript_window_for_layout(
        &self,
        layout: &crate::document::DocumentLayout,
        transcript_line_count: usize,
        manual_scroll: bool,
    ) -> Option<(usize, usize)> {
        let document_offset = if manual_scroll {
            self.document_runtime
                .viewport_state
                .resolve_offset_for_current_geometry(
                    layout,
                    self.document_viewport_height(),
                    self.width,
                )
        } else {
            self.document_runtime.viewport_state.resolved_offset()
        };
        let line_indices = self.document_viewport_line_indices_for_mode(
            layout,
            document_offset,
            self.document_runtime.viewport_state.follow_bottom(),
            manual_scroll,
        );

        let mut start = None;
        let mut count = 0usize;
        for line_index in line_indices {
            if line_index >= transcript_line_count {
                if start.is_some() {
                    break;
                }
                continue;
            }

            start.get_or_insert(line_index);
            count += 1;
        }

        start.map(|start| (start, count))
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
