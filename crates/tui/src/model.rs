use std::fmt::Write as _;
use std::time::{Duration, Instant};
use std::{collections::BTreeMap, rc::Rc};

use mo_core::{
    acp::AcpAgentIdentity,
    envinfo,
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    phrases::StatusPhraseOrder,
    session::{
        RuntimeAvailableCommand, RuntimeModelConfig, RuntimeTerminalSnapshot, RuntimeToolActivity,
        RuntimeToolActivityUpdate,
    },
};
use ratatui::Frame;

use super::{
    HeroOptions, ReasoningDisplayMode, Sender,
    acp::{AcpDebugPanelState, AcpPanelState, PendingAcpPermission},
    backtrack::BacktrackState,
    composer::Composer,
    composer_mouse::PendingComposerCursorClick,
    document::{
        LayoutCache, RestoreState, TailLayoutCache, TranscriptCache, ViewportCache, ViewportState,
        offset_viewport_line_indices,
    },
    external_editor::ExternalEditorLaunch,
    file_picker::{FILE_PICKER_POPUP_MAX_HEIGHT, FILE_PICKER_POPUP_MIN_HEIGHT, FilePickerState},
    file_search::FileSearchCache,
    model_panel::ModelPanelState,
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
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

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(super) hero_options: HeroOptions,
    pub(super) style_mode: StyleMode,
    pub(super) status_line_items: Vec<StatusLineItem>,
    pub(super) status_line_2_items: Vec<StatusLineItem>,
    pub(super) external_editor: Vec<String>,
    pub(super) external_editor_hint: String,
    pub(super) external_editor_helper_enabled: bool,
    pub(super) acp_agent_servers: Vec<String>,
    pub(super) acp_agent_identities: BTreeMap<String, AcpAgentIdentity>,
    pub(super) selected_acp_agent: Option<String>,
    pub(super) acp_current_model: Option<String>,
    pub(super) acp_model_config_id: Option<String>,
    pub(super) pending_acp_model_rollback: Option<AcpModelSelectionRollback>,
    pub(super) acp_available_commands_by_agent: BTreeMap<String, Vec<RuntimeAvailableCommand>>,
    pub(super) acp_panel: AcpPanelState,
    pub(super) acp_debug_panel: AcpDebugPanelState,
    pub(super) model_catalog: ModelCatalog,
    pub(super) selected_model: Option<ModelSelection>,
    pub(super) requires_model_selection: bool,
    pub(super) model_panel: ModelPanelState,
    pub(super) tool_approval_panel: ToolApprovalPanelState,
    pub(super) tool_approval_panel_revision: usize,
    pub(super) transcript_overlay: Option<crate::transcript_overlay::TranscriptOverlayState>,
    pub(super) backtrack: BacktrackState,
    pub(super) pending_acp_permission: Option<PendingAcpPermission>,
    pub(super) acp_terminal_snapshots: BTreeMap<String, RuntimeTerminalSnapshot>,
    pub(super) stream_activity: Option<StreamActivityState>,
    pub(super) runtime_response_buffer: RuntimeResponseBuffer,
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

/// `RuntimeResponseBuffer` 暂存非 ACP runtime 的流式文本，直到工具调用等语义边界出现。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RuntimeResponseBuffer {
    content: String,
    reasoning_content: String,
    reasoning_started_at: Option<Instant>,
}

impl RuntimeResponseBuffer {
    fn is_empty(&self) -> bool {
        self.content.is_empty() && self.reasoning_content.is_empty()
    }

    fn push_content(&mut self, content: &str) {
        self.content.push_str(content);
    }

    fn push_reasoning_content(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        self.reasoning_started_at.get_or_insert_with(Instant::now);
        self.reasoning_content.push_str(content);
    }

    fn clear(&mut self) {
        self.content.clear();
        self.reasoning_content.clear();
        self.reasoning_started_at = None;
    }

    fn take(&mut self) -> Option<BufferedRuntimeResponse> {
        let content = std::mem::take(&mut self.content);
        let reasoning_content = if self.reasoning_content.is_empty() {
            self.reasoning_started_at = None;
            None
        } else {
            Some(std::mem::take(&mut self.reasoning_content))
        };
        let reasoning_duration = reasoning_content
            .as_ref()
            .and_then(|_| self.reasoning_started_at.take())
            .map(|started_at| Instant::now().saturating_duration_since(started_at));

        if content.is_empty() && reasoning_content.is_none() {
            return None;
        }

        Some(BufferedRuntimeResponse {
            content,
            reasoning_content,
            reasoning_duration,
        })
    }

    fn take_with_final(
        &mut self,
        final_content: String,
        final_reasoning_content: Option<String>,
        final_reasoning_duration: Option<Duration>,
    ) -> Option<BufferedRuntimeResponse> {
        let mut response = self.take().unwrap_or_else(|| BufferedRuntimeResponse {
            content: String::new(),
            reasoning_content: None,
            reasoning_duration: None,
        });

        response.content = reconcile_buffered_text_with_final(response.content, final_content);
        let (reasoning_content, reasoning_duration) = reconcile_buffered_reasoning_with_final(
            response.reasoning_content,
            response.reasoning_duration,
            final_reasoning_content,
            final_reasoning_duration,
        );
        response.reasoning_content = reasoning_content;
        response.reasoning_duration = reasoning_duration;

        if response.content.is_empty() && response.reasoning_content.is_none() {
            return None;
        }

        Some(response)
    }
}

fn reconcile_buffered_text_with_final(buffered: String, final_content: String) -> String {
    if buffered.is_empty() {
        return final_content;
    }
    if final_content.is_empty() {
        return buffered;
    }
    if final_content.starts_with(&buffered) {
        return final_content;
    }

    buffered
}

fn reconcile_buffered_reasoning_with_final(
    buffered: Option<String>,
    buffered_duration: Option<Duration>,
    final_content: Option<String>,
    final_duration: Option<Duration>,
) -> (Option<String>, Option<Duration>) {
    match (buffered, final_content) {
        (None, None) => (None, None),
        (None, Some(content)) => (Some(content), final_duration),
        (Some(content), None) => (Some(content), buffered_duration),
        (Some(buffered), Some(final_content)) => {
            if final_content.is_empty() {
                return (Some(buffered), buffered_duration);
            }
            if final_content.starts_with(&buffered) {
                return (Some(final_content), final_duration.or(buffered_duration));
            }
            (Some(buffered), buffered_duration)
        }
    }
}

struct BufferedRuntimeResponse {
    content: String,
    reasoning_content: Option<String>,
    reasoning_duration: Option<Duration>,
}

/// `AcpModelSelectionRollback` 保存 ACP 模型切换请求确认前的本地选择状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcpModelSelectionRollback {
    agent_id: String,
    selected_model: Option<ModelSelection>,
    acp_current_model: Option<String>,
}

/// `SelectionRuntimeState` 收口 selection 与拖拽自动滚动的运行态。
#[derive(Debug, Clone)]
pub(super) struct SelectionRuntimeState {
    pub(super) selection: SelectionState,
    pub(super) click: SelectionClickState,
    pub(super) version: usize,
    pub(super) auto_scroll_direction: AutoScrollDirection,
    pub(super) auto_scroll_token: usize,
    pub(super) auto_scroll_mouse: MousePosition,
    pub(super) auto_scroll_deadline: Option<Instant>,
}

impl Default for SelectionRuntimeState {
    fn default() -> Self {
        Self {
            selection: SelectionState::default(),
            click: SelectionClickState::default(),
            version: 0,
            auto_scroll_direction: AutoScrollDirection::None,
            auto_scroll_token: 0,
            auto_scroll_mouse: MousePosition::default(),
            auto_scroll_deadline: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct PendingReasoningToggleClick {
    pub(super) item_index: usize,
    pub(super) column: u16,
    pub(super) row: u16,
    pub(super) active: bool,
}

/// `DocumentRuntimeState` 收口统一文档 viewport、cache 与手动滚动状态。
#[derive(Debug, Clone, Default)]
pub(super) struct DocumentRuntimeState {
    pub(super) viewport_y: usize,
    pub(super) viewport_state: ViewportState,
    pub(super) transcript_cache: TranscriptCache,
    pub(super) tail_layout_cache: TailLayoutCache,
    pub(super) layout_cache: LayoutCache,
    pub(super) viewport_cache: ViewportCache,
    pub(super) follow_bottom: bool,
    pub(super) manual_scroll: bool,
    pub(super) restore: RestoreState,
}

/// `NoticeState` 收口短暂提示、滚动提示、外部编辑器提示与退出确认。
#[derive(Debug, Clone, Default)]
pub(super) struct NoticeState {
    pub(super) status_text: String,
    pub(super) status_token: usize,
    pub(super) status_deadline: Option<Instant>,
    pub(super) history_scroll_indicator_token: usize,
    pub(super) history_scroll_indicator_deadline: Option<Instant>,
    pub(super) external_editor_helper_visible: bool,
    pub(super) external_editor_helper_token: usize,
    pub(super) external_editor_helper_deadline: Option<Instant>,
    pub(super) exit_confirmation_deadline: Option<Instant>,
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
    pub acp_agent_servers: Vec<String>,
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
            acp_agent_servers: Vec::new(),
            model_catalog: ModelCatalog::default(),
            selected_model: None,
            requires_model_selection: false,
            status_phrases: default_status_phrases(),
            status_phrase_order: StatusPhraseOrder::Random,
        }
    }
}

pub(crate) fn acp_model_provider_id(agent_id: &str) -> String {
    format!("acp:{agent_id}")
}

fn acp_model_label(name: &str, value: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        value.to_string()
    } else {
        name.to_string()
    }
}

fn acp_model_entry_description(name: &str, value: &str) -> Option<String> {
    let label = acp_model_label(name, value);
    (label != value).then_some(label)
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
    pub fn new(hero_options: HeroOptions) -> Self {
        Self::new_with_options(hero_options, ModelOptions::default())
    }

    /// `new_with_style_mode` 创建并初始化带指定样式模式的 TUI 模型。
    pub fn new_with_style_mode(hero_options: HeroOptions, style_mode: StyleMode) -> Self {
        Self::new_with_options(
            hero_options,
            ModelOptions {
                style_mode,
                ..ModelOptions::default()
            },
        )
    }

    /// `new_with_options` 创建并初始化带显式选项的 TUI 模型。
    pub fn new_with_options(hero_options: HeroOptions, options: ModelOptions) -> Self {
        let palette = default_palette();
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.append_hero(hero_options.clone());
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
            hero_options,
            style_mode,
            status_line_items: status_line_items.clone(),
            status_line_2_items,
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            acp_agent_servers: options.acp_agent_servers,
            acp_agent_identities: BTreeMap::new(),
            selected_acp_agent: None,
            acp_current_model: None,
            acp_model_config_id: None,
            pending_acp_model_rollback: None,
            acp_available_commands_by_agent: BTreeMap::new(),
            acp_panel: AcpPanelState::default(),
            acp_debug_panel: AcpDebugPanelState::default(),
            model_catalog: options.model_catalog,
            selected_model,
            requires_model_selection: options.requires_model_selection,
            model_panel: ModelPanelState::default(),
            tool_approval_panel: ToolApprovalPanelState::default(),
            tool_approval_panel_revision: 1,
            transcript_overlay: None,
            backtrack: BacktrackState::default(),
            pending_acp_permission: None,
            acp_terminal_snapshots: BTreeMap::new(),
            stream_activity: None,
            runtime_response_buffer: RuntimeResponseBuffer::default(),
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

    /// `selected_acp_agent` 返回本次 TUI 会话中用户选择的 ACP Agent。
    pub fn selected_acp_agent(&self) -> Option<&str> {
        self.selected_acp_agent.as_deref()
    }

    /// `apply_acp_agent_identity` 记录 ACP agent 初始化后上报的展示信息。
    pub(crate) fn apply_acp_agent_identity(
        &mut self,
        agent_id: impl Into<String>,
        identity: AcpAgentIdentity,
    ) {
        let agent_id = agent_id.into();
        self.acp_agent_identities.insert(agent_id, identity);
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn acp_agent_display_label(&self, agent_id: &str) -> String {
        self.acp_agent_identities
            .get(agent_id)
            .filter(|identity| identity.has_agent_info())
            .map(AcpAgentIdentity::display_label)
            .unwrap_or_else(|| agent_id.to_string())
    }

    /// `apply_acp_available_commands` 记录当前 ACP session 上报的动态斜杠命令。
    pub(crate) fn apply_acp_available_commands<C>(
        &mut self,
        agent_id: impl Into<String>,
        commands: impl IntoIterator<Item = C>,
    ) where
        C: Into<RuntimeAvailableCommand>,
    {
        let agent_id = agent_id.into();
        let commands = commands.into_iter().map(Into::into).collect::<Vec<_>>();
        if commands.is_empty() {
            self.acp_available_commands_by_agent.remove(&agent_id);
        } else {
            self.acp_available_commands_by_agent
                .insert(agent_id, commands);
        }
        self.sync_command_panel_navigation();
    }

    /// `clear_acp_available_commands` 清理指定 ACP agent 的动态斜杠命令。
    pub(crate) fn clear_acp_available_commands(&mut self, agent_id: &str) {
        self.acp_available_commands_by_agent.remove(agent_id);
        self.sync_command_panel_navigation();
    }

    /// `selected_acp_available_commands` 返回当前活跃 ACP session 的动态命令。
    pub(crate) fn selected_acp_available_commands(&self) -> &[RuntimeAvailableCommand] {
        self.selected_acp_agent
            .as_deref()
            .and_then(|agent_id| self.acp_available_commands_by_agent.get(agent_id))
            .map(Vec::as_slice)
            .unwrap_or(&[])
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
        transcript.append_hero(self.hero_options.clone());
        self.transcript = transcript;
        self.composer.clear();
        self.acp_panel = AcpPanelState::default();
        self.acp_debug_panel = AcpDebugPanelState::default();
        self.model_panel = ModelPanelState::default();
        self.tool_approval_panel = ToolApprovalPanelState::default();
        self.tool_approval_panel_revision = self.tool_approval_panel_revision.saturating_add(1);
        self.backtrack = BacktrackState::default();
        self.pending_acp_permission = None;
        self.acp_terminal_snapshots.clear();
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

    pub(crate) fn append_acp_response_from_runtime(
        &mut self,
        content: impl Into<String>,
        reasoning_content: Option<String>,
        reasoning_duration: Option<std::time::Duration>,
    ) {
        self.append_runtime_response_from_runtime(content, reasoning_content, reasoning_duration);
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
            self.append_runtime_response_from_runtime(
                response.content,
                response.reasoning_content,
                response.reasoning_duration,
            );
        }
    }

    pub(crate) fn flush_runtime_response_buffer_with_final(
        &mut self,
        final_content: String,
        final_reasoning_content: Option<String>,
        final_reasoning_duration: Option<Duration>,
    ) {
        if let Some(response) = self.runtime_response_buffer.take_with_final(
            final_content,
            final_reasoning_content,
            final_reasoning_duration,
        ) {
            self.append_runtime_response_from_runtime(
                response.content,
                response.reasoning_content,
                response.reasoning_duration,
            );
        }
    }

    pub(crate) fn clear_runtime_response_buffer(&mut self) {
        self.runtime_response_buffer.clear();
    }

    pub(crate) fn activate_acp_model_scope(&mut self, agent_id: &str) {
        let provider_id = acp_model_provider_id(agent_id);
        self.pending_acp_model_rollback = None;
        self.model_catalog = ModelCatalog::new(vec![ModelProvider::acp(
            provider_id,
            format!("ACP: {agent_id}"),
            Vec::new(),
        )]);
        self.selected_model = None;
        self.acp_current_model = None;
        self.acp_model_config_id = None;
        self.bump_status_line_revision();
        self.sync_model_panel_to_selection();
    }

    pub(crate) fn apply_acp_model_config(&mut self, agent_id: &str, config: RuntimeModelConfig) {
        let provider_id = acp_model_provider_id(agent_id);
        let display_name = format!("ACP: {agent_id}");
        let mut entries = config
            .options
            .into_iter()
            .filter_map(|option| {
                let value = option.value.trim().to_string();
                if value.is_empty() {
                    return None;
                }
                let description = acp_model_entry_description(&option.name, &value);
                Some(ModelEntry::new(value, description, ModelSource::Acp))
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            entries.push(ModelEntry::new(
                config.current_value.clone(),
                acp_model_entry_description(&config.current_name, &config.current_value),
                ModelSource::Acp,
            ));
        }
        let current_label = acp_model_label(&config.current_name, &config.current_value);

        self.model_catalog = ModelCatalog::new(vec![ModelProvider::acp(
            provider_id.clone(),
            display_name,
            entries,
        )]);
        self.selected_model = Some(ModelSelection::new(provider_id, config.current_value));
        self.set_acp_current_model(Some(current_label));
        self.acp_model_config_id = config.config_id;
        self.commit_pending_acp_model_change(agent_id);
        self.sync_model_panel_to_selection();
    }

    /// `begin_pending_acp_model_change` 记录 ACP 模型切换乐观更新前的本地状态。
    pub(crate) fn begin_pending_acp_model_change(&mut self, agent_id: &str) {
        if self
            .pending_acp_model_rollback
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent_id == agent_id)
        {
            return;
        }

        self.pending_acp_model_rollback = Some(AcpModelSelectionRollback {
            agent_id: agent_id.to_string(),
            selected_model: self.selected_model.clone(),
            acp_current_model: self.acp_current_model.clone(),
        });
    }

    /// `commit_pending_acp_model_change` 在 agent 确认切换成功后丢弃回滚快照。
    pub(crate) fn commit_pending_acp_model_change(&mut self, agent_id: &str) {
        if self
            .pending_acp_model_rollback
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent_id == agent_id)
        {
            self.pending_acp_model_rollback = None;
        }
    }

    /// `rollback_pending_acp_model_change` 在 ACP 模型切换失败后恢复旧选择。
    pub(crate) fn rollback_pending_acp_model_change(&mut self, agent_id: &str) {
        let Some(snapshot) = self.pending_acp_model_rollback.take() else {
            return;
        };
        if snapshot.agent_id != agent_id {
            self.pending_acp_model_rollback = Some(snapshot);
            return;
        }

        let changed = self.selected_model != snapshot.selected_model
            || self.acp_current_model != snapshot.acp_current_model;
        self.selected_model = snapshot.selected_model;
        self.acp_current_model = snapshot.acp_current_model;
        if changed {
            self.bump_status_line_revision();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
            }
        }
        self.sync_model_panel_to_selection();
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
        for snapshot in self
            .acp_terminal_snapshots
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
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
        for snapshot in self
            .acp_terminal_snapshots
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
            let _ = self
                .transcript_mut()
                .set_runtime_terminal_snapshot(snapshot);
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
        self.acp_terminal_snapshots
            .insert(snapshot.terminal_id.clone(), snapshot.clone());
        if !self
            .transcript_mut()
            .set_runtime_terminal_snapshot(snapshot)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn has_active_acp_background_terminals(&self) -> bool {
        self.acp_terminal_snapshots
            .values()
            .any(is_active_acp_terminal_snapshot)
    }

    pub(crate) fn acp_background_terminal_summary_text(&self) -> String {
        let active = self
            .acp_terminal_snapshots
            .values()
            .filter(|snapshot| is_active_acp_terminal_snapshot(snapshot))
            .collect::<Vec<_>>();
        if active.is_empty() {
            return "No background terminals running.".to_string();
        }

        let mut summary = String::from("Background terminals:");
        for snapshot in active {
            let command = snapshot
                .command
                .as_deref()
                .filter(|command| !command.trim().is_empty())
                .unwrap_or("terminal");
            let _ = write!(summary, "\n- {command}");
            if let Some(cwd) = snapshot.cwd.as_deref().filter(|cwd| !cwd.trim().is_empty()) {
                let _ = write!(summary, "\n  {cwd}");
            }
            if let Some(output) = acp_terminal_recent_output(&snapshot.output) {
                let _ = write!(summary, "\n  {output}");
            }
        }
        summary
    }

    pub(crate) fn set_acp_tool_call_approval_suspended_from_runtime(
        &mut self,
        item_index: usize,
        suspended: bool,
    ) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .set_acp_tool_call_approval_suspended(item_index, suspended)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn set_acp_tool_call_permission_waiting_from_runtime(
        &mut self,
        item_index: usize,
        waiting: bool,
    ) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .set_acp_tool_call_permission_waiting(item_index, waiting)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn mark_acp_tool_calls_failed_from_runtime(
        &mut self,
        item_indices: impl IntoIterator<Item = usize>,
        message: &str,
    ) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .mark_acp_tool_calls_failed(item_indices, message)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        true
    }

    pub(crate) fn mark_acp_tool_call_rejected_from_runtime(&mut self, item_index: usize) -> bool {
        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        if !self
            .transcript_mut()
            .mark_acp_tool_call_rejected(item_index)
        {
            return false;
        }
        self.refresh_status_line_after_transcript_change();
        self.sync_transcript_render();
        self.document_runtime.follow_bottom = true;
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
        let acp_panel = self.current_inline_acp_panel_render_result();
        let tool_approval_panel = self.current_inline_tool_approval_panel_render_result();
        if status_line.has_content
            || status_line_2.has_content
            || command_panel.has_content
            || model_panel.has_content
            || acp_panel.has_content
            || tool_approval_panel.has_content
        {
            if self.document_runtime.follow_bottom && !self.document_runtime.manual_scroll {
                let panel_rows = command_panel.lines.len()
                    + model_panel.lines.len()
                    + acp_panel.lines.len()
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

fn is_active_acp_terminal_snapshot(snapshot: &RuntimeTerminalSnapshot) -> bool {
    snapshot.exit_status.is_none() && !snapshot.released
}

fn acp_terminal_recent_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .find_map(|line| {
            let line = line.trim();
            (!line.is_empty()).then(|| line.to_string())
        })
        .map(|line| {
            const MAX_RECENT_OUTPUT_WIDTH: usize = 96;
            crate::status_line::truncate_display_width_with_ellipsis(&line, MAX_RECENT_OUTPUT_WIDTH)
        })
}

#[cfg(test)]
mod tests;
