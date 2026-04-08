use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::Frame;

use crate::envinfo;

use super::{
    HeroOptions,
    composer::Composer,
    composer_mouse::PendingComposerCursorClick,
    document::{
        LayoutCache, RestoreState, TailLayoutCache, TranscriptCache, ViewportCache, ViewportState,
        offset_viewport_line_indices,
    },
    external_editor::ExternalEditorLaunch,
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
    pub(super) command_panel_selected: usize,
    pub(super) command_panel_scroll: usize,
    pub(super) copy_on_mouse_selection_release: bool,
    pub(super) swap_enter_and_send: bool,
    pub(super) ctrl_c_clears_input: bool,
    pub(super) selection: SelectionState,
    pub(super) selection_click: SelectionClickState,
    pub(super) pending_composer_cursor_click: PendingComposerCursorClick,
    pub(super) selection_version: usize,
    pub(super) selection_auto_scroll_direction: AutoScrollDirection,
    pub(super) selection_auto_scroll_token: usize,
    pub(super) selection_auto_scroll_mouse: MousePosition,
    pub(super) selection_auto_scroll_deadline: Option<Instant>,
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
    pub(super) document_viewport_y: usize,
    pub(super) document_viewport_state: ViewportState,
    pub(super) document_transcript_cache: TranscriptCache,
    pub(super) document_tail_layout_cache: TailLayoutCache,
    pub(super) document_layout_cache: LayoutCache,
    pub(super) document_viewport_cache: ViewportCache,
    pub(super) has_palette: bool,
    pub(super) has_window: bool,
    pub(super) has_dark_background: bool,
    pub(super) follow_bottom: bool,
    pub(super) manual_document_scroll: bool,
    pub(super) manual_scroll_restore: RestoreState,
    pub(super) status_notice_text: String,
    pub(super) status_notice_token: usize,
    pub(super) status_notice_deadline: Option<Instant>,
    pub(super) history_scroll_indicator_token: usize,
    pub(super) history_scroll_indicator_deadline: Option<Instant>,
    pub(super) transcript_refine_token: usize,
    pub(super) transcript_refine_deadline: Option<Instant>,
    pub(super) transcript_refine_ranges: Vec<(usize, usize)>,
    pub(super) transcript_refine_cursor: usize,
    pub(super) external_editor_helper_visible: bool,
    pub(super) external_editor_helper_token: usize,
    pub(super) external_editor_helper_deadline: Option<Instant>,
    pub(super) exit_confirmation_deadline: Option<Instant>,
    pub(super) status_line_revision: usize,
    quitting: bool,
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

const TRANSCRIPT_REFINE_INTERVAL: Duration = Duration::from_millis(8);

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

        Self {
            style_mode,
            status_line_items: status_line_items.clone(),
            external_editor: options.external_editor,
            external_editor_hint: options.external_editor_hint,
            external_editor_helper_enabled: options.show_external_editor_helper,
            command_panel_selected: 0,
            command_panel_scroll: 0,
            copy_on_mouse_selection_release: options.copy_on_mouse_selection_release,
            swap_enter_and_send: options.swap_enter_and_send,
            ctrl_c_clears_input: options.ctrl_c_clears_input,
            selection: SelectionState::default(),
            selection_click: SelectionClickState::default(),
            pending_composer_cursor_click: PendingComposerCursorClick::default(),
            selection_version: 0,
            selection_auto_scroll_direction: AutoScrollDirection::None,
            selection_auto_scroll_token: 0,
            selection_auto_scroll_mouse: MousePosition::default(),
            selection_auto_scroll_deadline: None,
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
            document_viewport_y: 0,
            document_viewport_state: ViewportState::default(),
            document_transcript_cache: TranscriptCache::default(),
            document_tail_layout_cache: TailLayoutCache::default(),
            document_layout_cache: LayoutCache::default(),
            document_viewport_cache: ViewportCache::default(),
            has_palette: false,
            has_window: false,
            has_dark_background: true,
            follow_bottom: true,
            manual_document_scroll: false,
            manual_scroll_restore: RestoreState::default(),
            status_notice_text: String::new(),
            status_notice_token: 0,
            status_notice_deadline: None,
            history_scroll_indicator_token: 0,
            history_scroll_indicator_deadline: None,
            transcript_refine_token: 0,
            transcript_refine_deadline: None,
            transcript_refine_ranges: Vec::new(),
            transcript_refine_cursor: 0,
            external_editor_helper_visible: false,
            external_editor_helper_token: 0,
            external_editor_helper_deadline: None,
            exit_confirmation_deadline: None,
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

    /// `next_timeout_deadline` 返回当前最早需要处理的内部超时。
    pub fn next_timeout_deadline(&self) -> Option<Instant> {
        [
            self.status_notice_deadline,
            self.external_editor_helper_deadline,
            self.history_scroll_indicator_deadline,
            self.transcript_refine_deadline,
            self.selection_auto_scroll_deadline,
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
        if palette_changed && self.selection.is_active() {
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
        if let Some(deadline) = self.status_notice_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::StatusNoticeTimeout {
                token: self.status_notice_token,
            });
        }

        if let Some(deadline) = self.history_scroll_indicator_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::HistoryScrollIndicatorTimeout {
                token: self.history_scroll_indicator_token,
            });
        }

        if let Some(deadline) = self.transcript_refine_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::TranscriptRefineTick {
                token: self.transcript_refine_token,
            });
        }

        if let Some(deadline) = self.external_editor_helper_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::ExternalEditorHelperTimeout {
                token: self.external_editor_helper_token,
            });
        }

        if let Some(deadline) = self.selection_auto_scroll_deadline
            && now >= deadline
        {
            return Some(super::AppEvent::SelectionAutoScrollTick {
                token: self.selection_auto_scroll_token,
            });
        }

        None
    }

    pub(crate) fn maybe_prepare_external_editor_launch(&mut self) -> Option<ExternalEditorLaunch> {
        self.prepare_external_editor_launch()
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
        if status_line.has_content || command_panel.has_content {
            if self.follow_bottom && !self.manual_document_scroll {
                let visible_height =
                    self.bottom_follow_composer_content_line_count(&status_line, &command_panel);
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
        self.document_transcript_cache = Default::default();
        self.document_layout_cache = Default::default();
        self.schedule_transcript_refinement();
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
            .manual_document_scroll
            .then(|| self.document_viewport_state.clone());
        let index = self
            .exactize_visible_transcript_window_until_stable(self.transcript_render.index.clone());
        if index == self.transcript_render.index {
            return;
        }

        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.document_transcript_cache = Default::default();
        self.document_layout_cache = Default::default();
        self.document_viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
        self.schedule_transcript_refinement();
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

    pub(super) fn schedule_transcript_refinement(&mut self) {
        let index = self.transcript_render.index.clone();
        let Some((start, count)) = self.current_visible_transcript_window_for_index(&index) else {
            self.transcript_refine_ranges.clear();
            self.transcript_refine_cursor = 0;
            self.transcript_refine_deadline = None;
            return;
        };
        let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
        let Some((exact_start, exact_end)) =
            index.item_range_for_line_window(start, count, overscan_lines)
        else {
            self.transcript_refine_ranges.clear();
            self.transcript_refine_cursor = 0;
            self.transcript_refine_deadline = None;
            return;
        };

        self.transcript_refine_ranges =
            build_transcript_refine_ranges(index.metrics.as_slice(), exact_start, exact_end);
        self.transcript_refine_cursor = 0;
        self.transcript_refine_token = self.transcript_refine_token.saturating_add(1);
        self.transcript_refine_deadline = (!self.transcript_refine_ranges.is_empty())
            .then_some(Instant::now() + TRANSCRIPT_REFINE_INTERVAL);
    }

    pub(crate) fn run_next_transcript_refinement_batch(&mut self) -> bool {
        let scheduled_cursor = self.transcript_refine_cursor;
        let scheduled_token = self.transcript_refine_token;
        let Some(&(start, end)) = self.transcript_refine_ranges.get(scheduled_cursor) else {
            self.transcript_refine_deadline = None;
            return false;
        };

        let preserved_viewport_state = self.preserved_viewport_state_for_transcript_refresh();
        self.transcript.exactize_item_range(start, end);
        let index = self.transcript.progressive_item_metrics_index();
        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.document_transcript_cache = Default::default();
        self.document_layout_cache = Default::default();
        self.document_viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);

        // viewport exactization 可能会在同步期间重建 refine 队列，此时必须保留新队列的游标。
        let schedule_preserved = self.transcript_refine_token == scheduled_token
            && self.transcript_refine_ranges.get(scheduled_cursor).copied() == Some((start, end));
        if !schedule_preserved {
            return true;
        }

        self.transcript_refine_cursor = scheduled_cursor + 1;
        self.transcript_refine_deadline = (self.transcript_refine_cursor
            < self.transcript_refine_ranges.len())
        .then_some(Instant::now() + TRANSCRIPT_REFINE_INTERVAL);
        true
    }

    pub(crate) fn drain_transcript_refinement_for_benchmark(&mut self) {
        while self.run_next_transcript_refinement_batch() {}
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
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let mut tail_rows = command_panel.lines.len();
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
            self.document_viewport_y,
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

        if self.document_viewport_state.manual_scroll() {
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

        let manual_scroll = self.document_viewport_state.manual_scroll();
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
            self.document_viewport_state
                .resolve_offset(layout, self.document_viewport_height())
        } else {
            self.document_viewport_state.resolved_offset()
        };
        let line_indices = self.document_viewport_line_indices_for_mode(
            layout,
            document_offset,
            self.document_viewport_state.follow_bottom(),
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

fn build_transcript_refine_ranges(
    metrics: &[crate::frontend::tui::transcript::TranscriptItemMetrics],
    exact_start: usize,
    exact_end: usize,
) -> Vec<(usize, usize)> {
    const TRANSCRIPT_REFINE_BATCH_ITEMS: usize = 32;

    if metrics.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::new();
    let mut left_end = exact_start.min(metrics.len());
    let mut right_start = exact_end.min(metrics.len());

    while left_end > 0 || right_start < metrics.len() {
        if right_start < metrics.len() {
            let right_end = right_start
                .saturating_add(TRANSCRIPT_REFINE_BATCH_ITEMS)
                .min(metrics.len());
            if metrics[right_start..right_end]
                .iter()
                .any(crate::frontend::tui::transcript::TranscriptItemMetrics::is_estimated)
            {
                ranges.push((right_start, right_end));
            }
            right_start = right_end;
        }

        if left_end > 0 {
            let left_start = left_end.saturating_sub(TRANSCRIPT_REFINE_BATCH_ITEMS);
            if metrics[left_start..left_end]
                .iter()
                .any(crate::frontend::tui::transcript::TranscriptItemMetrics::is_estimated)
            {
                ranges.push((left_start, left_end));
            }
            left_end = left_start;
        }
    }

    ranges
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::frontend::tui::{AppEvent, Sender, StyleMode, document::DocumentAnchorRegion};

    fn progressive_exactization_fixture() -> Model {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..40 {
            let content = match index % 4 {
                0 => {
                    format!("# Overview {index} alpha beta gamma delta epsilon zeta eta theta iota")
                }
                1 => format!(
                    "```rust\nfn helper_{index}() {{ println!(\"alpha beta gamma delta epsilon zeta eta theta iota\"); }}\n```"
                ),
                2 => format!(
                    "| key | value |\n| --- | --- |\n| alpha beta gamma {index} | delta epsilon zeta eta theta |\n| iota kappa lambda | mu nu xi omicron pi |"
                ),
                _ => format!(
                    "__init__ item {index} keeps markdown emphasis and heading-like text wrapped across the viewport"
                ),
            };
            model
                .transcript_mut()
                .append_message(Sender::Assistant, content);
        }
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();
        model
    }

    fn idle_refinement_fixture() -> Model {
        let mut model = progressive_exactization_fixture();
        model
            .composer_mut()
            .set_text_for_test("draft line one\ndraft line two\ndraft line three");
        model.composer_mut().move_to_begin_for_test();
        model.sync_composer_height();
        model
    }

    fn apply_scrolled_offset(model: &mut Model, offset: usize, manual_scroll: bool) {
        let layout = model.build_document_layout();
        let composer_offset = model.current_composer_viewport_offset(&layout, offset);
        model.apply_document_viewport_position(
            &layout,
            offset,
            composer_offset,
            false,
            manual_scroll,
        );
    }

    #[test]
    fn overflowing_document_bottom_slice_keeps_full_draft_height() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);
        model.composer_mut().set_text_for_test("1\n2\n3");
        model.sync_composer_height();
        model.sync_document_viewport_to_bottom();

        let layout = model.build_document_layout();
        assert_eq!(layout.composer_line_count, 3);

        let viewport = model.build_document_viewport(&layout);
        let rendered = viewport.plain_lines.clone();
        assert_eq!(
            rendered,
            vec![
                String::new(),
                "┃ 1".to_string(),
                "┃ 2".to_string(),
                "┃ 3".to_string(),
            ]
        );
    }

    #[test]
    fn transcript_plain_items_use_assistant_markdown_render_path() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "# Overview of the API");

        assert_eq!(model.transcript_plain_items(), vec!["Overview of the API"]);
    }

    #[test]
    fn height_only_resize_keeps_transcript_render_stable() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "alpha\nbeta\ngamma\ndelta");
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);
        model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
        model.sync_composer_height();

        let before_render_version = model.transcript_render_version;
        let before_render = Rc::clone(&model.transcript_render);
        let before_composer_height = model.composer.visible_height();

        model.set_window(20, 8);

        assert_eq!(
            model.transcript_render_version, before_render_version,
            "height-only resize should not trigger a transcript rerender"
        );
        assert!(
            Rc::ptr_eq(&before_render, &model.transcript_render),
            "height-only resize should keep reusing the current transcript render result"
        );
        assert!(
            model.composer.visible_height() > before_composer_height,
            "height-only resize should still update the tail/composer layout"
        );
    }

    #[test]
    fn setting_the_same_palette_keeps_transcript_render_stable() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "alpha\nbeta");
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);

        let before_render_version = model.transcript_render_version;
        let before_render = Rc::clone(&model.transcript_render);

        model.set_palette(default_palette(), true);

        assert_eq!(
            model.transcript_render_version, before_render_version,
            "setting the same palette should not trigger a transcript rerender"
        );
        assert!(
            Rc::ptr_eq(&before_render, &model.transcript_render),
            "setting the same palette should keep the existing transcript render result"
        );
    }

    #[test]
    fn current_visible_transcript_window_matches_actual_viewport_line_indices() {
        #[derive(Clone, Copy)]
        enum TailState {
            Plain,
            StatusLine,
            CommandPanel,
        }

        for (name, style_mode, height, composer_text, tail_state) in [
            ("plain draft", StyleMode::Ms, 6, "draft", TailState::Plain),
            (
                "status line with tall draft",
                StyleMode::Ms,
                6,
                "1\n2\n3\n4\n5\n6\n7\n8",
                TailState::StatusLine,
            ),
            (
                "command panel",
                StyleMode::Ms,
                6,
                "/",
                TailState::CommandPanel,
            ),
            ("framed draft", StyleMode::Cc, 3, "draft", TailState::Plain),
            (
                "framed tall draft",
                StyleMode::Cc,
                4,
                "1\n2\n3\n4\n5\n6",
                TailState::Plain,
            ),
        ] {
            let mut model = Model::new_with_style_mode(HeroOptions::default(), style_mode);
            model.transcript_mut().clear();
            model.transcript_mut().set_gap(0);
            for index in 0..48 {
                model
                    .transcript_mut()
                    .append_message(Sender::Assistant, format!("item {index}"));
            }
            model.set_window(24, height);
            model.set_palette(default_palette(), true);
            match tail_state {
                TailState::Plain => {}
                TailState::StatusLine => {
                    model.status_line_items = vec![StatusLineItem::GitBranch];
                    model.git_branch = "main".to_string();
                }
                TailState::CommandPanel => {}
            }
            model.composer_mut().set_text_for_test(composer_text);
            model.sync_command_panel_navigation();
            model.sync_composer_height();
            model.sync_transcript_render();
            model.sync_document_viewport_to_bottom();

            let layout = model.build_document_layout();
            let visible_transcript_indices = model
                .document_viewport_line_indices(&layout)
                .into_iter()
                .filter(|line_index| *line_index < layout.transcript_line_count)
                .collect::<Vec<_>>();
            let expected_window = visible_transcript_indices
                .first()
                .copied()
                .map(|start| (start, visible_transcript_indices.len()));

            assert_eq!(
                model.current_visible_transcript_window(layout.transcript_line_count),
                expected_window,
                "{name} should derive the warmed transcript window from the actual viewport line indices"
            );
        }
    }

    #[test]
    fn sync_transcript_render_evicts_warmed_transcript_blocks_during_metrics_only_refresh() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.set_window(24, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("item {index}"));
        }

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "metrics-only sync should keep transcript blocks cold before any viewport materialization"
        );

        model.document_transcript_cache = Default::default();
        let _snapshot = model.current_document_transcript_snapshot();
        assert!(
            !model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "document transcript snapshot should prewarm the visible transcript neighborhood"
        );

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "metrics-only refresh should evict warmed transcript blocks from the previous viewport snapshot"
        );
    }

    #[test]
    fn current_visible_transcript_window_reresolves_manual_scroll_viewport_after_resize() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "target item");
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "tail item");
        model.set_window(24, 4);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        let layout = model.build_document_layout();
        let target_document_line = (0..layout.line_count())
            .find(|&line_index| {
                layout.line_anchor_at(line_index).is_some_and(|anchor| {
                    anchor.region == DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == 1
                })
            })
            .expect("target item should exist in the initial transcript layout");
        let document_offset = target_document_line;
        model.apply_document_viewport_position(&layout, document_offset, 0, false, true);
        let preserved_viewport_state = model.current_document_viewport_state();

        model.set_window(12, 4);

        let transcript_line_count = model.transcript.item_metrics_index().line_count;
        let resized_layout = model.build_document_layout();
        let resized_target_document_line = (0..resized_layout.line_count())
            .find(|&line_index| {
                resized_layout
                    .line_anchor_at(line_index)
                    .is_some_and(|anchor| {
                        anchor.region == DocumentAnchorRegion::Transcript
                            && anchor.transcript.item_index == 1
                    })
            })
            .expect("target item should still exist after resize");
        let expected_offset = preserved_viewport_state
            .resolve_offset(&resized_layout, model.document_viewport_height());
        let stale_offset = preserved_viewport_state.resolved_offset();
        let expected_window = model
            .document_viewport_line_indices_for_mode(
                &resized_layout,
                expected_offset,
                preserved_viewport_state.follow_bottom(),
                preserved_viewport_state.manual_scroll(),
            )
            .into_iter()
            .filter(|line_index| *line_index < transcript_line_count)
            .collect::<Vec<_>>();
        let stale_window = model
            .document_viewport_line_indices_for_mode(
                &resized_layout,
                stale_offset,
                preserved_viewport_state.follow_bottom(),
                preserved_viewport_state.manual_scroll(),
            )
            .into_iter()
            .filter(|line_index| *line_index < transcript_line_count)
            .collect::<Vec<_>>();
        let expected_window = expected_window
            .first()
            .copied()
            .map(|start| (start, expected_window.len()));

        assert_ne!(
            expected_offset, stale_offset,
            "test fixture should force manual-scroll restore to resolve a different offset after reflow (before={target_document_line}, after={resized_target_document_line})"
        );
        assert_ne!(
            stale_window
                .first()
                .copied()
                .map(|start| (start, stale_window.len())),
            expected_window,
            "test fixture should expose a mismatch between stale and re-resolved viewport windows"
        );
        assert_eq!(
            model.current_visible_transcript_window(transcript_line_count),
            expected_window,
            "manual-scroll prewarm should follow the re-resolved viewport that will be restored after resize"
        );
    }

    #[test]
    fn current_visible_transcript_window_rebuilds_manual_scroll_index_when_reflow_keeps_line_count()
    {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "target item");
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "tail item");
        model.set_window(24, 4);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        let layout = model.build_document_layout();
        let target_document_line = (0..layout.line_count())
            .find(|&line_index| {
                layout.line_anchor_at(line_index).is_some_and(|anchor| {
                    anchor.region == DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == 1
                })
            })
            .expect("target item should exist in the initial transcript layout");
        model.apply_document_viewport_position(&layout, target_document_line, 0, false, true);

        let preserved_viewport_state = model.current_document_viewport_state();
        let stale_index = model.transcript_render.index.clone();
        model.width = 12;
        model.transcript.set_width(12);
        model.composer.set_width(12);
        let resized_index = model.transcript.progressive_item_metrics_index();
        let resized_layout = model.document_layout_for_transcript_index(resized_index.clone());
        let expected_offset = preserved_viewport_state
            .resolve_offset(&resized_layout, model.document_viewport_height());
        let expected_window_lines = model
            .document_viewport_line_indices_for_mode(
                &resized_layout,
                expected_offset,
                preserved_viewport_state.follow_bottom(),
                preserved_viewport_state.manual_scroll(),
            )
            .into_iter()
            .filter(|line_index| *line_index < resized_index.line_count)
            .collect::<Vec<_>>();
        let expected_window = expected_window_lines
            .first()
            .copied()
            .map(|start| (start, expected_window_lines.len()));
        let forced_stale_index = crate::frontend::tui::transcript::TranscriptItemMetricsIndex {
            line_count: resized_index.line_count,
            ..stale_index
        };
        model.transcript_render = Rc::new(index_only_render_result(forced_stale_index));

        assert_eq!(
            model.current_visible_transcript_window(resized_index.line_count),
            expected_window,
            "line_count equality alone should not let manual-scroll reuse a stale transcript index after reflow"
        );
    }

    #[test]
    fn sync_transcript_render_keeps_transcript_blocks_cold_when_document_viewport_is_unavailable() {
        #[derive(Clone, Copy)]
        enum ViewportState {
            MissingWindow,
            ZeroHeight,
        }

        for viewport_state in [ViewportState::MissingWindow, ViewportState::ZeroHeight] {
            let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
            model.set_window(24, 6);
            model.set_palette(default_palette(), true);
            model.transcript_mut().clear();
            model.transcript_mut().set_gap(0);
            for index in 0..96 {
                model
                    .transcript_mut()
                    .append_message(Sender::Assistant, format!("item {index}"));
            }

            model.sync_transcript_render();
            assert!(
                model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "Phase E sync_transcript_render should stop after metrics rebuild even while the viewport is still available"
            );

            model.document_transcript_cache = Default::default();
            let _snapshot = model.current_document_transcript_snapshot();
            assert!(
                !model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "test fixture should warm transcript blocks before making the viewport unavailable"
            );

            match viewport_state {
                ViewportState::MissingWindow => {
                    model.has_window = false;
                }
                ViewportState::ZeroHeight => {
                    model.height = 0;
                }
            }

            assert_eq!(model.document_viewport_height(), 0);
            let transcript_line_count = model.transcript.item_metrics_index().line_count;
            assert_eq!(
                model.current_visible_transcript_window(transcript_line_count),
                None,
                "unavailable viewport should not report any transcript line as visible"
            );

            model.sync_transcript_render();
            assert!(
                model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "sync_transcript_render should keep transcript blocks cold when no viewport is available"
            );

            model.document_transcript_cache = Default::default();
            let _snapshot = model.current_document_transcript_snapshot();
            assert!(
                model
                    .transcript
                    .cached_screen_blocks_snapshot()
                    .borrow()
                    .is_empty(),
                "document transcript snapshots should not retain transcript blocks when no viewport is available"
            );
        }
    }

    #[test]
    fn sync_transcript_render_keeps_current_viewport_exact_without_settling_entire_transcript() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);

        model.sync_transcript_render();

        let index = model.transcript_render.index.clone();
        let (start, count) = model
            .current_visible_transcript_window(index.line_count)
            .expect("bottom-follow viewport should expose a visible transcript window");
        let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
        let (start_position, end_position) = index
            .summary_positions_for_line_window(start, count, overscan_lines)
            .expect("visible transcript window should resolve to summary positions");
        let exact_items = index.visible_items[start_position..=end_position]
            .iter()
            .map(|position| position.item_index)
            .collect::<Vec<_>>();

        assert!(
            !exact_items.is_empty(),
            "test fixture should expose at least one visible transcript item"
        );
        assert!(
            exact_items
                .iter()
                .all(|item_index| index.metrics[*item_index].is_exact()),
            "visible transcript window should be exact after sync_transcript_render"
        );
        assert!(
            index
                .metrics
                .iter()
                .enumerate()
                .any(|(item_index, metrics)| {
                    !exact_items.contains(&item_index) && metrics.is_estimated()
                }),
            "progressive sync should leave non-visible transcript history estimated instead of settling the whole transcript"
        );
    }

    #[test]
    fn build_document_layout_exactizes_a_newly_scrolled_transcript_window() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        let tail_layout = model.build_document_layout();
        model.apply_document_viewport_position(&tail_layout, 0, 0, false, true);

        let _top_layout = model.build_document_layout();
        let index = model.transcript_render.index.clone();
        let (start, count) = model
            .current_visible_transcript_window(index.line_count)
            .expect("manually scrolled viewport should expose a visible transcript window");
        let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);

        assert!(
            index.line_window_is_exact(start, count, overscan_lines),
            "building a layout for a newly scrolled viewport should exactize that transcript window before document rendering"
        );
        assert!(
            index
                .metrics
                .iter()
                .enumerate()
                .any(|(item_index, metrics)| { item_index > 16 && metrics.is_estimated() }),
            "scroll-driven exactization should stay local instead of settling the whole transcript"
        );
    }

    #[test]
    fn build_document_layout_stable_exactization_loop_keeps_visible_window_exact() {
        let base = progressive_exactization_fixture();
        let layout = base.clone().build_document_layout();
        let max_offset = layout
            .line_count()
            .saturating_sub(base.document_viewport_height());

        for manual_scroll in [false, true] {
            for offset in 0..=max_offset {
                let mut model = base.clone();
                apply_scrolled_offset(&mut model, offset, manual_scroll);

                let index = model.transcript_render.index.clone();
                let Some((start, count)) =
                    model.current_visible_transcript_window_for_index(&index)
                else {
                    continue;
                };
                let overscan_lines =
                    crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
                if index.line_window_is_exact(start, count, overscan_lines) {
                    continue;
                }

                let index = model.exactize_visible_transcript_window_until_stable(index);
                let Some((next_start, next_count)) =
                    model.current_visible_transcript_window_for_index(&index)
                else {
                    continue;
                };
                let next_overscan_lines =
                    crate::frontend::tui::transcript::viewport_overscan_line_budget(next_count);
                assert!(
                    index.line_window_is_exact(next_start, next_count, next_overscan_lines),
                    "stable exactization should converge the visible transcript window to exact metrics at offset {offset} (manual_scroll={manual_scroll})"
                );
            }
        }
    }

    #[test]
    fn exactize_line_window_keeps_manual_scroll_window_local_after_reflow() {
        let mut model = progressive_exactization_fixture();
        let offset = 10;
        apply_scrolled_offset(&mut model, offset, true);

        let index = model.transcript_render.index.clone();
        let (start, count) = model
            .current_visible_transcript_window_for_index(&index)
            .expect("manual-scroll viewport should expose a visible transcript window");
        let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
        assert!(
            !index.line_window_is_exact(start, count, overscan_lines),
            "test fixture should keep manual offset {offset} on the progressive path before render-time exactization"
        );

        let expected_item_range = index
            .item_range_for_line_window(start, count, overscan_lines)
            .expect("visible transcript window should resolve to an item range");
        let actual_item_range = model
            .transcript
            .exactize_line_window(start, count, overscan_lines)
            .expect("exactization should cover the visible transcript items");

        assert_eq!(
            actual_item_range, expected_item_range,
            "exactize_line_window should only exactize the item range resolved for the requested line window before reflow"
        );
    }

    #[test]
    fn build_document_layout_keeps_manual_scroll_viewport_stable_without_exactization_reflow() {
        let base = progressive_exactization_fixture();
        let layout = base.clone().build_document_layout();
        let max_offset = layout
            .line_count()
            .saturating_sub(base.document_viewport_height());

        for offset in 0..=max_offset {
            let mut model = base.clone();
            apply_scrolled_offset(&mut model, offset, true);
            let preserved_viewport_state = model.document_viewport_state.clone();

            let layout = model.build_document_layout();
            let expected_offset =
                preserved_viewport_state.resolve_offset(&layout, model.document_viewport_height());
            let viewport = model.build_document_viewport(&layout);

            assert_eq!(
                model.document_viewport_y, expected_offset,
                "manual-scroll viewport should stay aligned with the preserved transcript anchor at offset {offset}"
            );
            assert_eq!(
                model.document_viewport_state.resolved_offset(),
                expected_offset,
                "viewport state should store the stable manual-scroll offset at offset {offset}"
            );
            assert_eq!(
                viewport.resolved_offset, expected_offset,
                "document viewport materialization should keep using the resolved manual-scroll offset at offset {offset}"
            );
        }
    }

    #[test]
    fn build_document_layout_resyncs_idle_viewport_after_exactization_reflow() {
        let base = idle_refinement_fixture();
        let layout = base.clone().build_document_layout();
        let max_offset = layout
            .line_count()
            .saturating_sub(base.document_viewport_height());
        let mut candidate = None;

        for offset in 0..=max_offset {
            let mut probe = base.clone();
            apply_scrolled_offset(&mut probe, offset, false);
            if probe.follow_bottom || probe.manual_document_scroll {
                continue;
            }

            let stale_offset = probe.document_viewport_state.resolved_offset();
            let mut exactized = probe.clone();
            let layout = exactized.build_document_layout();
            let cursor_hidden_with_stale_offset = layout.cursor_y < stale_offset
                || layout.cursor_y
                    >= stale_offset.saturating_add(exactized.document_viewport_height());

            let mut expected = exactized.clone();
            expected.sync_document_viewport_for_composer_cursor();
            if cursor_hidden_with_stale_offset && expected.document_viewport_y != stale_offset {
                candidate = Some(offset);
                break;
            }
        }

        let offset = candidate.expect(
            "test fixture should expose a non-follow-bottom viewport whose stale offset hides the composer cursor after render-time exactization",
        );

        let mut model = base;
        apply_scrolled_offset(&mut model, offset, false);

        let mut expected = model.clone();
        let _ = expected.build_document_layout();
        expected.sync_document_viewport_for_composer_cursor();

        let layout = model.build_document_layout();
        let viewport = model.build_document_viewport(&layout);

        assert_eq!(
            model.document_viewport_y, expected.document_viewport_y,
            "render-time exactization should immediately rerun the idle viewport cursor sync"
        );
        assert_eq!(
            model.composer.viewport_offset(),
            expected.composer.viewport_offset(),
            "composer viewport should stay aligned with the cursor-tracking sync after exactization"
        );
        assert_eq!(
            model.document_viewport_state.resolved_offset(),
            expected.document_viewport_y,
            "viewport state should store the cursor-tracking offset after exactization"
        );
        assert_eq!(
            viewport.resolved_offset, expected.document_viewport_y,
            "document viewport materialization should use the cursor-tracking offset after exactization"
        );
        assert!(
            layout.cursor_y >= viewport.resolved_offset
                && layout.cursor_y
                    < viewport
                        .resolved_offset
                        .saturating_add(model.document_viewport_height()),
            "render-time exactization should leave the active composer cursor inside the visible document viewport"
        );
    }

    #[test]
    fn transcript_refinement_batch_restarts_from_new_queue_after_viewport_requeues_work() {
        let base = progressive_exactization_fixture();
        let layout = base.clone().build_document_layout();
        let max_offset = layout
            .line_count()
            .saturating_sub(base.document_viewport_height());
        let mut candidate = None;

        'search: for manual_scroll in [false, true] {
            for offset in 0..=max_offset {
                let mut probe = base.clone();
                apply_scrolled_offset(&mut probe, offset, manual_scroll);
                if probe.transcript_refine_ranges.is_empty() {
                    continue;
                }

                let scheduled_token = probe.transcript_refine_token;
                if !probe.run_next_transcript_refinement_batch() {
                    continue;
                }

                if probe.transcript_refine_token != scheduled_token
                    && !probe.transcript_refine_ranges.is_empty()
                {
                    candidate = Some((offset, manual_scroll));
                    break 'search;
                }
            }
        }

        let (offset, manual_scroll) = candidate.expect(
            "test fixture should expose a refinement batch whose viewport sync requeues follow-up work",
        );

        let mut model = base;
        apply_scrolled_offset(&mut model, offset, manual_scroll);
        let scheduled_token = model.transcript_refine_token;

        assert!(
            model.run_next_transcript_refinement_batch(),
            "candidate should still execute a refinement batch"
        );
        assert_ne!(
            model.transcript_refine_token, scheduled_token,
            "candidate should reschedule refinement work while syncing the viewport after the batch"
        );
        assert!(
            !model.transcript_refine_ranges.is_empty(),
            "rescheduled refinement queue should still contain follow-up work"
        );
        assert_eq!(
            model.transcript_refine_cursor, 0,
            "requeued refinement work should restart from the new queue instead of skipping its first batch"
        );
        assert!(
            model.transcript_refine_deadline.is_some(),
            "requeued refinement queue should keep a pending deadline for its next batch"
        );
    }

    #[test]
    fn transcript_refinement_ticks_eventually_settle_remaining_estimated_history() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        assert!(
            model
                .transcript_render
                .index
                .metrics
                .iter()
                .any(|metrics| metrics.is_estimated()),
            "test fixture should begin in a mixed-quality state before refinement ticks"
        );

        for _ in 0..32 {
            let Some(deadline) = model.transcript_refine_deadline else {
                break;
            };
            let Some(AppEvent::TranscriptRefineTick { token }) = model.timeout_event(deadline)
            else {
                break;
            };
            let _ = model.update(AppEvent::TranscriptRefineTick { token });
            if model
                .transcript_render
                .index
                .metrics
                .iter()
                .all(|metrics| !metrics.is_estimated())
            {
                break;
            }
        }

        assert!(
            model
                .transcript_render
                .index
                .metrics
                .iter()
                .all(|metrics| !metrics.is_estimated()),
            "refinement ticks should eventually settle the remaining estimated transcript history"
        );
    }

    #[test]
    fn transcript_refinement_deadline_yields_before_next_batch() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..160 {
            model.transcript_mut().append_message(
                Sender::Assistant,
                format!(
                    "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
                ),
            );
        }
        model.set_window(18, 6);
        model.set_palette(default_palette(), true);
        model.sync_transcript_render();

        assert!(
            model.transcript_refine_ranges.len() > 1,
            "test fixture should schedule more than one refinement batch"
        );
        assert!(
            model
                .transcript_refine_deadline
                .is_some_and(|deadline| deadline > Instant::now()),
            "initial refinement deadline should be deferred so the runner can poll for input before background work continues"
        );

        assert!(model.run_next_transcript_refinement_batch());
        assert!(
            model.transcript_refine_cursor < model.transcript_refine_ranges.len(),
            "test fixture should still have pending refinement work after the first batch"
        );
        assert!(
            model
                .transcript_refine_deadline
                .is_some_and(|deadline| deadline > Instant::now()),
            "follow-up refinement batches should also defer their deadline instead of immediately rearming another timeout tick"
        );
    }

    #[test]
    fn transcript_refinement_batch_keeps_idle_non_follow_bottom_cursor_visible() {
        let base = idle_refinement_fixture();
        let layout = base.clone().build_document_layout();
        let max_offset = layout
            .line_count()
            .saturating_sub(base.document_viewport_height());
        let mut candidate = None;

        for offset in 0..=max_offset {
            let mut model = base.clone();
            apply_scrolled_offset(&mut model, offset, false);
            if model.follow_bottom || model.manual_document_scroll {
                continue;
            }
            let Some(&(start, end)) = model.transcript_refine_ranges.first() else {
                continue;
            };

            let preserved_viewport_state = model.current_document_viewport_state();
            model.transcript.exactize_item_range(start, end);
            let index = model.transcript.progressive_item_metrics_index();
            model.transcript_render = Rc::new(index_only_render_result(index));
            model.transcript_render_version += 1;
            model.document_transcript_cache = Default::default();
            model.document_layout_cache = Default::default();
            model.document_viewport_cache = Default::default();

            let layout = model.build_document_layout();
            let restored_offset =
                preserved_viewport_state.resolve_offset(&layout, model.document_viewport_height());
            let restored_cursor_visible = layout.cursor_y >= restored_offset
                && layout.cursor_y
                    < restored_offset.saturating_add(model.document_viewport_height());

            model.sync_document_viewport_for_composer_cursor();
            let expected_offset = model.document_viewport_y;

            if !restored_cursor_visible && expected_offset != restored_offset {
                candidate = Some(offset);
                break;
            }
        }

        let offset = candidate.expect(
            "test fixture should expose an idle non-follow-bottom viewport whose restored transcript anchor would hide the composer cursor after exactization",
        );

        let mut model = base;
        apply_scrolled_offset(&mut model, offset, false);
        let Some(&(start, end)) = model.transcript_refine_ranges.first() else {
            panic!("candidate should keep at least one pending refinement batch");
        };

        let mut expected = model.clone();
        expected.transcript.exactize_item_range(start, end);
        let index = expected.transcript.progressive_item_metrics_index();
        expected.transcript_render = Rc::new(index_only_render_result(index));
        expected.transcript_render_version += 1;
        expected.document_transcript_cache = Default::default();
        expected.document_layout_cache = Default::default();
        expected.document_viewport_cache = Default::default();
        expected.sync_document_viewport_for_composer_cursor();

        assert!(
            model.run_next_transcript_refinement_batch(),
            "candidate should still execute a refinement batch"
        );
        let actual_layout = model.build_document_layout();
        assert_eq!(
            model.document_viewport_y, expected.document_viewport_y,
            "background refinement should keep the composer cursor in view instead of restoring a transcript anchor from ordinary editing mode"
        );
        assert_eq!(
            model.composer.viewport_offset(),
            expected.composer.viewport_offset(),
            "composer viewport should match the cursor-tracking path after refinement reflow"
        );
        assert!(
            actual_layout.cursor_y >= model.document_viewport_y
                && actual_layout.cursor_y
                    < model
                        .document_viewport_y
                        .saturating_add(model.document_viewport_height()),
            "background refinement should leave the active composer cursor inside the visible document viewport"
        );
    }
}
