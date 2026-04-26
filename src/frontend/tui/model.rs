use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::Frame;

use crate::envinfo;
use crate::runtime::models::{ModelCatalog, ModelSelection};

use super::{
    HeroOptions, Sender,
    acp_activity::AcpActivityState,
    acp_panel::AcpPanelState,
    acp_permission::PendingAcpPermission,
    composer::Composer,
    composer_mouse::PendingComposerCursorClick,
    document::{
        LayoutCache, RestoreState, TailLayoutCache, TranscriptCache, ViewportCache, ViewportState,
        offset_viewport_line_indices,
    },
    external_editor::ExternalEditorLaunch,
    model_panel::ModelPanelState,
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
    status_line::{StatusLineItem, StatusLineRenderResult},
    style_mode::StyleMode,
    theme::{TerminalPalette, default_palette},
    transcript::{RenderResult, Transcript, TranscriptEstimateBreakdown, index_only_render_result},
    view,
};

/// `Model` 表示交互式 TUI 应用的状态。
#[derive(Debug, Clone)]
pub struct Model {
    pub(super) style_mode: StyleMode,
    pub(super) status_line_items: Vec<StatusLineItem>,
    pub(super) external_editor: Vec<String>,
    pub(super) external_editor_hint: String,
    pub(super) external_editor_helper_enabled: bool,
    pub(super) acp_agent_servers: Vec<String>,
    pub(super) selected_acp_agent: Option<String>,
    pub(super) acp_panel: AcpPanelState,
    pub(super) model_catalog: ModelCatalog,
    pub(super) selected_model: Option<ModelSelection>,
    pub(super) requires_model_selection: bool,
    pub(super) model_panel: ModelPanelState,
    pub(super) pending_acp_permission: Option<PendingAcpPermission>,
    pub(super) acp_activity: Option<AcpActivityState>,
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) copy_on_mouse_selection_release: bool,
    pub(super) swap_enter_and_send: bool,
    pub(super) ctrl_c_clears_input: bool,
    pub(super) selection_runtime: SelectionRuntimeState,
    pub(super) pending_composer_cursor_click: PendingComposerCursorClick,
    pub(super) git_branch: String,
    pub(super) current_dir: String,
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
    pub external_editor: Vec<String>,
    pub external_editor_hint: String,
    pub show_external_editor_helper: bool,
    pub copy_on_mouse_selection_release: bool,
    pub swap_enter_and_send: bool,
    pub ctrl_c_clears_input: bool,
    pub acp_agent_servers: Vec<String>,
    pub model_catalog: ModelCatalog,
    pub selected_model: Option<ModelSelection>,
    pub requires_model_selection: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        Self {
            style_mode: StyleMode::default(),
            status_line_items: Vec::new(),
            external_editor: Vec::new(),
            external_editor_hint: String::new(),
            show_external_editor_helper: true,
            copy_on_mouse_selection_release: false,
            swap_enter_and_send: false,
            ctrl_c_clears_input: true,
            acp_agent_servers: Vec::new(),
            model_catalog: ModelCatalog::default(),
            selected_model: None,
            requires_model_selection: false,
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
        transcript.append_hero(hero_options);
        let transcript_render = Rc::new(index_only_render_result(
            transcript.progressive_item_metrics_index(),
        ));
        let style_mode = options.style_mode.normalized();
        let status_line_items = options.status_line_items;
        let selected_model = options
            .selected_model
            .filter(|selection| options.model_catalog.contains_selection(selection));

        Self {
            style_mode,
            status_line_items: status_line_items.clone(),
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            acp_agent_servers: options.acp_agent_servers,
            selected_acp_agent: None,
            acp_panel: AcpPanelState::default(),
            model_catalog: options.model_catalog,
            selected_model,
            requires_model_selection: options.requires_model_selection,
            model_panel: ModelPanelState::default(),
            pending_acp_permission: None,
            acp_activity: None,
            command_panel_selected: 0,
            command_panel_scroll: 0,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            selection_runtime: SelectionRuntimeState::default(),
            pending_composer_cursor_click: PendingComposerCursorClick::default(),
            git_branch: resolve_initial_git_branch(&status_line_items),
            current_dir: resolve_initial_current_dir(&status_line_items),
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

    pub(crate) fn sync_composer_height(&mut self) {
        let full_height = self.composer.full_height().max(1);
        let mut viewport_height = if !self.has_window || self.height == 0 {
            full_height
        } else {
            full_height.min(self.height.max(1))
        };

        let status_line = self.current_status_line_render_result();
        let command_panel = self.current_inline_command_panel_render_result();
        let model_panel = self.current_inline_model_panel_render_result();
        let acp_panel = self.current_inline_acp_panel_render_result();
        if status_line.has_content
            || command_panel.has_content
            || model_panel.has_content
            || acp_panel.has_content
        {
            if self.document_runtime.follow_bottom && !self.document_runtime.manual_scroll {
                let visible_height = self.bottom_follow_composer_content_line_count(
                    &status_line,
                    &command_panel,
                    &model_panel,
                    &acp_panel,
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
        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.invalidate_document_viewport_cache();
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
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
        mut index: crate::frontend::tui::transcript::TranscriptItemMetricsIndex,
    ) -> crate::frontend::tui::transcript::TranscriptItemMetricsIndex {
        let mut remaining_items = index.metrics.len();
        while remaining_items > 0 {
            let Some((start, count)) = self.current_visible_transcript_window_for_index(&index)
            else {
                break;
            };
            let overscan_lines =
                crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                break;
            }

            let Some((start_item, end_item)) =
                self.transcript
                    .exactize_line_window(start, count, overscan_lines)
            else {
                break;
            };
            let next_index = self.transcript.progressive_item_metrics_index();
            if next_index == index {
                break;
            }
            index = next_index;
            remaining_items = remaining_items.saturating_sub(end_item.saturating_sub(start_item));
        }

        index
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
        command_panel: &super::command_panel::CommandPanelRenderResult,
        model_panel: &super::model_panel::ModelPanelRenderResult,
        acp_panel: &super::acp_panel::AcpPanelRenderResult,
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let acp_activity = self.current_acp_activity_render_result();
        let mut tail_rows =
            command_panel.lines.len() + model_panel.lines.len() + acp_panel.lines.len();
        if acp_activity.has_content {
            tail_rows += 1;
        }
        if status_line.has_content {
            tail_rows += status_line.gap_before + 1;
        }
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
        index: &crate::frontend::tui::transcript::TranscriptItemMetricsIndex,
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
        layout: &crate::frontend::tui::document::DocumentLayout,
        transcript_line_count: usize,
        manual_scroll: bool,
    ) -> Option<(usize, usize)> {
        let document_offset = if manual_scroll {
            self.document_runtime
                .viewport_state
                .resolve_offset(layout, self.document_viewport_height())
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

fn resolve_initial_git_branch(items: &[StatusLineItem]) -> String {
    if items
        .iter()
        .any(|item| matches!(item, StatusLineItem::GitBranch))
    {
        envinfo::git_branch()
    } else {
        String::new()
    }
}

fn resolve_initial_current_dir(items: &[StatusLineItem]) -> String {
    if items
        .iter()
        .any(|item| matches!(item, StatusLineItem::CurrentDir))
    {
        envinfo::short_work_dir()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests;
