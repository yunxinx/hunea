use std::{
    cell::OnceCell,
    collections::BTreeMap,
    mem,
    rc::Rc,
    time::{Duration, Instant},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::activity::{
    RuntimeExecuteFooterLine, RuntimeExecuteFooterStatus, RuntimeExecuteTranscriptBlock,
    RuntimeToolActivityDetailBlock, ToolActivityGroupFamily, active_marker_visible_at,
    execute_tool_call_shell_command, is_execute_like_tool_call, is_list_dir_tool_call,
    is_runtime_read_tool_activity, list_dir_tool_call_title_chunks,
    runtime_read_tool_activity_title_chunks, runtime_tool_activity_detail_blocks,
    runtime_tool_activity_diff_header_chunks, runtime_tool_activity_display_title,
    runtime_tool_activity_location_suffix, runtime_tool_activity_status_color,
    runtime_write_tool_activity_title_chunks, style_for_color,
};
use super::approval::{ParsedToolResultLine, looks_like_shell_command, style_core_result_line};
#[cfg(test)]
use super::diff::DiffBudget;
use super::diff::{
    DiffDisplay, DiffPresentations, RuntimeDiffDetailLine, effective_diff_display,
    has_same_diff_presentation_inputs, runtime_diff_line_prefix,
    runtime_tool_activity_diff_gutter_style, runtime_tool_activity_diff_line_fill_style,
    runtime_tool_activity_diff_segment_style, runtime_tool_activity_has_diff_content,
};
use super::exploration::{
    coalesce_adjacent_target_display_lines, exploration_display_lines, exploration_group_family,
    failed_tool_call_detail_text, is_groupable_exploration_tool_call,
    standalone_exploration_tool_call, wrap_exploration_display_line,
    wrap_failed_exploration_detail_line,
};
use super::state::{
    active_marker_started_at_for_body, apply_runtime_tool_activity_update,
    runtime_tool_activity_source_byte_len, tool_result_render_cache_key,
};
use crate::transcript::markdown_highlight::HighlightChunk;
use crate::{
    display_width::display_width,
    runtime::tool_activity_preview::is_runtime_write_tool_activity,
    styled_text::{line_to_plain_text, lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        markdown_highlight::{
            highlight_code_chunks, wrap_highlight_chunks, wrap_highlight_chunks_soft,
        },
        wrap_prompt_visual_lines,
    },
};
use runtime_domain::session::{
    RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
};

const TOOL_RESULT_PREFIX: &str = "● ";
const TOOL_RESULT_CONTINUATION_PREFIX: &str = "  ";
const TOOL_EXPLORATION_PREFIX: &str = "● ";
pub(super) const TOOL_EXPLORATION_BRANCH_PREFIX: &str = "  └ ";
pub(super) const TOOL_EXPLORATION_CHILD_PREFIX: &str = "    ";
const TOOL_ACTIVITY_DETAIL_PREFIX: &str = "  └ ";
const TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX: &str = "    ";
pub(crate) const TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL: Duration = Duration::from_millis(600);
pub(super) const TOOL_ACTIVITY_COMPACT_EDGE_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolResultKind {
    Ran,
    Rejected,
}

/// `ToolActivityRenderMode` 控制工具活动在主界面与 transcript overlay 中的详略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolActivityRenderMode {
    Compact,
    Detailed,
    DebugDetailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum ToolResultBody {
    Approval {
        content: String,
        kind: ToolResultKind,
    },
    RuntimeToolActivity(RuntimeToolActivity),
    Exploration(Vec<RuntimeToolActivity>),
}

fn is_finished_execute_like_tool_call(call: &RuntimeToolActivity) -> bool {
    matches!(
        call.status,
        RuntimeToolActivityStatus::Completed | RuntimeToolActivityStatus::Failed
    ) && is_execute_like_tool_call(call)
}

fn runtime_finished_execute_title(title: &str) -> String {
    title
        .strip_prefix("Run ")
        .map(str::trim_start)
        .filter(|command| !command.is_empty())
        .unwrap_or(title)
        .to_string()
}

/// `DiffPresentationCache` 只缓存由 Diff content 派生的稳定语义结果。
/// cache 状态不属于 item 的值语义；clone 通过 `Rc` 保留同一 content revision 的计算产物。
#[derive(Debug, Clone, Default)]
struct DiffPresentationCache {
    state: OnceCell<DiffPresentationCacheState>,
}

/// 无 Diff 活动只记录枚举状态；确有 Diff 时才分配并共享 presentation。
#[derive(Debug, Clone)]
enum DiffPresentationCacheState {
    NoDiff,
    Ready(Rc<DiffPresentations>),
}

impl DiffPresentationCacheState {
    fn presentations(&self) -> Option<&DiffPresentations> {
        match self {
            Self::NoDiff => None,
            Self::Ready(presentations) => Some(presentations),
        }
    }
}

impl DiffPresentationCache {
    fn get_or_build(&self, call: &RuntimeToolActivity) -> Option<&DiffPresentations> {
        self.get_or_build_with(call, || DiffPresentations::build_for_content_revision(call))
    }

    #[cfg(test)]
    fn get_or_build_with_budget(
        &self,
        call: &RuntimeToolActivity,
        budget: DiffBudget,
    ) -> Option<&DiffPresentations> {
        self.get_or_build_with(call, || DiffPresentations::build(call, budget))
    }

    fn get_or_build_with(
        &self,
        call: &RuntimeToolActivity,
        build: impl FnOnce() -> DiffPresentations,
    ) -> Option<&DiffPresentations> {
        self.state
            .get_or_init(|| {
                if runtime_tool_activity_has_diff_content(call) {
                    DiffPresentationCacheState::Ready(Rc::new(build()))
                } else {
                    DiffPresentationCacheState::NoDiff
                }
            })
            .presentations()
    }

    fn clear(&mut self) {
        self.state.take();
    }
}

/// `ToolResultItem` 表示只用于 TUI 展示的工具活动，不参与模型上下文。
#[derive(Debug, Clone)]
pub(crate) struct ToolResultItem {
    body: ToolResultBody,
    diff_presentation_cache: DiffPresentationCache,
    render_mode: ToolActivityRenderMode,
    diff_display: DiffDisplay,
    render_cache_key: u64,
    active_marker_started_at: Option<Instant>,
    exploration_open: bool,
    approval_suspended: bool,
    permission_waiting: bool,
    terminal_snapshots: BTreeMap<String, RuntimeTerminalSnapshot>,
}

impl PartialEq for ToolResultItem {
    fn eq(&self, other: &Self) -> bool {
        self.body == other.body
            && self.render_mode == other.render_mode
            && self.diff_display == other.diff_display
            && self.render_cache_key == other.render_cache_key
            && self.active_marker_started_at == other.active_marker_started_at
            && self.exploration_open == other.exploration_open
            && self.approval_suspended == other.approval_suspended
            && self.permission_waiting == other.permission_waiting
            && self.terminal_snapshots == other.terminal_snapshots
    }
}

impl Eq for ToolResultItem {}

impl ToolResultItem {
    /// `new` 创建一条工具审批结果展示项。
    pub(crate) fn new(content: impl Into<String>, kind: ToolResultKind) -> Self {
        let content = content.into();
        Self::from_body(
            ToolResultBody::Approval { content, kind },
            ToolActivityRenderMode::Compact,
        )
    }

    /// `from_runtime_tool_activity` 创建一条 runtime tool activity 展示项。
    pub(crate) fn from_runtime_tool_activity(
        call: impl Into<RuntimeToolActivity>,
        render_mode: ToolActivityRenderMode,
    ) -> Self {
        let call = call.into();
        Self::from_body(ToolResultBody::RuntimeToolActivity(call), render_mode)
    }

    pub(crate) fn from_exploration_tool_activity(
        call: impl Into<RuntimeToolActivity>,
        render_mode: ToolActivityRenderMode,
    ) -> Option<Self> {
        let call = call.into();
        if render_mode == ToolActivityRenderMode::DebugDetailed {
            return Some(Self::from_runtime_tool_activity(call, render_mode));
        }

        is_groupable_exploration_tool_call(&call)
            .then(|| Self::from_body(ToolResultBody::Exploration(vec![call]), render_mode))
    }

    fn from_body(body: ToolResultBody, render_mode: ToolActivityRenderMode) -> Self {
        let approval_suspended = false;
        let permission_waiting = false;
        let terminal_snapshots = BTreeMap::new();
        let exploration_open = matches!(body, ToolResultBody::Exploration(_));
        let diff_display = DiffDisplay::default();
        let render_cache_key = tool_result_render_cache_key(
            &body,
            render_mode,
            exploration_open,
            approval_suspended,
            permission_waiting,
            &terminal_snapshots,
            diff_display,
        );
        let active_marker_started_at =
            active_marker_started_at_for_body(&body, &terminal_snapshots).then_some(Instant::now());
        Self {
            body,
            diff_presentation_cache: DiffPresentationCache::default(),
            render_mode,
            diff_display,
            render_cache_key,
            active_marker_started_at,
            exploration_open,
            approval_suspended,
            permission_waiting,
            terminal_snapshots,
        }
    }

    pub(crate) fn is_exploration_group(&self) -> bool {
        matches!(self.body, ToolResultBody::Exploration(_))
    }

    /// `render_lines` 将工具审批结果渲染为带颜色的文本行。
    pub(crate) fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        self.wrapped_styled_lines_with_active_marker_visible(width, palette)
    }

    /// `marker_now` 驱动活动 marker 的动画/静态显示（reduced-motion 下可为冻结时间）。
    pub(crate) fn render_lines_at(
        &self,
        width: u16,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        self.wrapped_styled_lines_at(width, palette, marker_now)
    }

    #[cfg(test)]
    pub(super) fn prebuild_diff_presentation_with_budget(&self, budget: DiffBudget) {
        let ToolResultBody::RuntimeToolActivity(call) = &self.body else {
            return;
        };
        self.diff_presentation_cache
            .get_or_build_with_budget(call, budget);
    }

    #[cfg(test)]
    pub(super) fn has_cached_no_diff_state(&self) -> bool {
        matches!(
            self.diff_presentation_cache.state.get(),
            Some(DiffPresentationCacheState::NoDiff)
        )
    }

    pub(crate) fn has_active_runtime_tool_activity(&self) -> bool {
        self.active_marker_started_at.is_some() && !self.is_compact_approval_suspended()
    }

    pub(crate) fn has_runtime_tool_activity_id(&self, tool_call_id: &str) -> bool {
        match &self.body {
            ToolResultBody::RuntimeToolActivity(call) => call.activity_id == tool_call_id,
            ToolResultBody::Exploration(calls) => calls
                .iter()
                .any(|call| call.activity_id.as_str() == tool_call_id),
            ToolResultBody::Approval { .. } => false,
        }
    }

    pub(crate) fn append_exploration_tool_activity(
        &mut self,
        call: impl Into<RuntimeToolActivity>,
    ) -> bool {
        let call = call.into();
        if !is_groupable_exploration_tool_call(&call) {
            return false;
        }

        let ToolResultBody::Exploration(calls) = &mut self.body else {
            return false;
        };
        let Some(existing_family) = exploration_group_family(calls) else {
            return false;
        };
        if exploration_group_family(std::slice::from_ref(&call)) != Some(existing_family) {
            return false;
        }

        calls.push(call);
        self.exploration_open = true;
        self.refresh_active_marker_started_at();
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn active_marker_started_at(&self) -> Option<Instant> {
        if self.is_compact_approval_suspended() {
            return None;
        }

        self.active_marker_started_at
    }

    fn wrapped_styled_lines_with_active_marker_visible(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let marker_now = self.active_marker_started_at.unwrap_or_else(Instant::now);
        self.wrapped_styled_lines_at(width, palette, marker_now)
    }

    fn wrapped_styled_lines_at(
        &self,
        width: u16,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        if self.is_compact_approval_suspended() {
            return Vec::new();
        }

        match &self.body {
            ToolResultBody::Approval { content, .. } => {
                self.approval_wrapped_styled_lines(content, width, palette)
            }
            ToolResultBody::RuntimeToolActivity(call) => {
                self.runtime_tool_activity_styled_lines_at(call, width, palette, marker_now)
            }
            ToolResultBody::Exploration(calls) => {
                self.exploration_styled_lines_at(calls, width, palette, marker_now)
            }
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
    pub(crate) fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let lines = self.render_lines(width, palette);
        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本内容。
    pub(crate) fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        lines_to_plain_text(&self.render_lines(width, palette))
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        match &self.body {
            ToolResultBody::Approval { content, .. } => content.len(),
            ToolResultBody::RuntimeToolActivity(call) => {
                runtime_tool_activity_source_byte_len(call)
                    + self
                        .terminal_snapshots
                        .values()
                        .map(|snapshot| snapshot.output.len())
                        .sum::<usize>()
            }
            ToolResultBody::Exploration(calls) => calls
                .iter()
                .map(runtime_tool_activity_source_byte_len)
                .sum(),
        }
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        let lines = self.wrapped_styled_lines_with_active_marker_visible(width, palette);
        let content_char_len = lines
            .iter()
            .map(|line| line_to_plain_text(line).len())
            .sum::<usize>();

        (lines.len(), content_char_len)
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let previous_metrics =
            previous_metrics.filter(|metrics| metrics.cache_key == self.render_cache_key);
        if let Some(metrics) = previous_metrics
            && metrics.is_valid
            && metrics.width == width
        {
            return TranscriptFastEstimate {
                content_line_count: metrics.content_line_count,
                content_char_len: metrics.content_char_len,
                kind: TranscriptEstimateKind::NonAssistant,
                ..TranscriptFastEstimate::default()
            };
        }

        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::NonAssistant,
            ..TranscriptFastEstimate::default()
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        _width: u16,
        _palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        Vec::new()
    }

    pub(crate) fn set_render_mode(&mut self, render_mode: ToolActivityRenderMode) -> bool {
        if self.render_mode == render_mode {
            return false;
        }

        self.render_mode = render_mode;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn with_diff_display(mut self, diff_display: DiffDisplay) -> Self {
        if self.diff_display == diff_display {
            return self;
        }
        self.diff_display = diff_display;
        self.refresh_render_cache_key();
        self
    }

    pub(crate) fn mark_exploration_complete(&mut self) -> bool {
        if !matches!(self.body, ToolResultBody::Exploration(_)) || !self.exploration_open {
            return false;
        }

        self.exploration_open = false;
        self.refresh_render_cache_key();
        true
    }

    pub(crate) fn set_runtime_terminal_snapshot(
        &mut self,
        snapshot: impl Into<RuntimeTerminalSnapshot>,
    ) -> bool {
        let snapshot = snapshot.into();
        let ToolResultBody::RuntimeToolActivity(call) = &self.body else {
            return false;
        };
        if !call.content.iter().any(|content| {
            matches!(content, RuntimeToolActivityContent::Terminal { terminal_id } if terminal_id == &snapshot.terminal_id)
        }) {
            return false;
        }

        if self
            .terminal_snapshots
            .get(&snapshot.terminal_id)
            .is_some_and(|current| current == &snapshot)
        {
            return false;
        }

        self.terminal_snapshots
            .insert(snapshot.terminal_id.clone(), snapshot);
        self.refresh_active_marker_started_at();
        self.refresh_render_cache_key();
        true
    }

    #[cfg(test)]
    pub(super) fn set_runtime_terminal_snapshot_for_test(
        &mut self,
        snapshot: impl Into<RuntimeTerminalSnapshot>,
    ) -> bool {
        self.set_runtime_terminal_snapshot(snapshot)
    }

    /// 应用 activity 增量，并返回满足展示状态不变量的替换项。
    ///
    /// exploration 不拥有 Diff presentation cache；更新引入 Diff 时按原顺序拆分，
    /// 由调用方原位替换 transcript 项，避免丢失同组的其他 activity。
    pub(crate) fn into_updated_runtime_tool_activity_items(
        mut self,
        update: impl Into<RuntimeToolActivityUpdate>,
    ) -> Option<Vec<Self>> {
        let update = update.into();
        let updated_activity_id = update.activity_id.clone();
        let update_status = update.status;
        let diff_presentation_inputs_changed =
            match &mut self.body {
                ToolResultBody::RuntimeToolActivity(call) => {
                    if call.activity_id != update.activity_id {
                        return None;
                    }
                    let inputs_changed = update.content.as_deref().is_some_and(|next| {
                        !has_same_diff_presentation_inputs(&call.content, next)
                    });
                    apply_runtime_tool_activity_update(
                        call,
                        update,
                        &mut self.permission_waiting,
                        &mut self.terminal_snapshots,
                    );
                    inputs_changed
                }
                ToolResultBody::Exploration(calls) => {
                    let call = calls
                        .iter_mut()
                        .find(|call| call.activity_id == update.activity_id)?;
                    let inputs_changed = update.content.as_deref().is_some_and(|next| {
                        !has_same_diff_presentation_inputs(&call.content, next)
                    });
                    let mut permission_waiting = false;
                    let mut terminal_snapshots = BTreeMap::new();
                    apply_runtime_tool_activity_update(
                        call,
                        update,
                        &mut permission_waiting,
                        &mut terminal_snapshots,
                    );
                    inputs_changed
                }
                ToolResultBody::Approval { .. } => return None,
            };
        if diff_presentation_inputs_changed {
            self.diff_presentation_cache.clear();
        }
        if update_status.is_some_and(|status| status != RuntimeToolActivityStatus::Pending) {
            self.approval_suspended = false;
            self.permission_waiting = false;
        }

        if matches!(
            &self.body,
            ToolResultBody::Exploration(calls)
                if calls.iter().any(runtime_tool_activity_has_diff_content)
        ) {
            return Some(self.into_diff_safe_items(&updated_activity_id));
        }

        self.refresh_active_marker_started_at();
        self.refresh_render_cache_key();
        Some(vec![self])
    }

    /// 将含 Diff 的失效 exploration 拆成连续 exploration 与普通 runtime item。
    fn into_diff_safe_items(self, updated_activity_id: &str) -> Vec<Self> {
        debug_assert!(matches!(&self.body, ToolResultBody::Exploration(_)));
        let Self {
            body: ToolResultBody::Exploration(calls),
            diff_presentation_cache: _,
            render_mode,
            diff_display,
            render_cache_key: _,
            active_marker_started_at,
            exploration_open,
            approval_suspended,
            permission_waiting,
            terminal_snapshots,
        } = self
        else {
            unreachable!("只有 exploration 需要按 Diff 边界规范化");
        };

        let mut bodies = Vec::new();
        let mut exploration_calls = Vec::new();
        for call in calls {
            if runtime_tool_activity_has_diff_content(&call) {
                if !exploration_calls.is_empty() {
                    bodies.push(ToolResultBody::Exploration(mem::take(
                        &mut exploration_calls,
                    )));
                }
                bodies.push(ToolResultBody::RuntimeToolActivity(call));
            } else {
                exploration_calls.push(call);
            }
        }
        if !exploration_calls.is_empty() {
            bodies.push(ToolResultBody::Exploration(exploration_calls));
        }

        debug_assert!(!bodies.is_empty());
        let last_body_index = bodies.len().saturating_sub(1);
        let mut terminal_snapshots = Some(terminal_snapshots);
        bodies
            .into_iter()
            .enumerate()
            .map(|(body_index, body)| {
                let is_updated_runtime = matches!(
                    &body,
                    ToolResultBody::RuntimeToolActivity(call)
                        if call.activity_id == updated_activity_id
                );
                let keeps_exploration_open = body_index == last_body_index
                    && exploration_open
                    && matches!(&body, ToolResultBody::Exploration(_));
                let mut item = Self::from_body(body, render_mode).with_diff_display(diff_display);
                item.exploration_open = keeps_exploration_open;

                if is_updated_runtime {
                    item.approval_suspended = approval_suspended;
                    item.permission_waiting = permission_waiting;
                    item.terminal_snapshots = terminal_snapshots.take().unwrap_or_default();
                }

                item.refresh_active_marker_started_at();
                if item.active_marker_started_at.is_some() {
                    // 拆分只改变展示归属，不应重启动画的时间原点。
                    item.active_marker_started_at =
                        active_marker_started_at.or(item.active_marker_started_at);
                }
                item.refresh_render_cache_key();
                item
            })
            .collect()
    }

    /// `set_approval_suspended` 临时隐藏正在等待审批的 compact 工具活动。
    pub(crate) fn set_approval_suspended(&mut self, is_suspended: bool) -> bool {
        if self.approval_suspended == is_suspended {
            return false;
        }

        self.approval_suspended = is_suspended;
        self.permission_waiting = is_suspended;
        self.refresh_active_marker_started_at();
        self.refresh_render_cache_key();
        true
    }

    fn refresh_active_marker_started_at(&mut self) {
        self.active_marker_started_at =
            active_marker_started_at_for_body(&self.body, &self.terminal_snapshots)
                .then(|| self.active_marker_started_at.unwrap_or_else(Instant::now));
    }

    fn approval_wrapped_styled_lines(
        &self,
        content: &str,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        content
            .split('\n')
            .enumerate()
            .flat_map(|(logical_line, content_line)| {
                self.wrap_logical_line(content_line, logical_line, width, palette)
            })
            .collect()
    }

    fn exploration_styled_lines_at(
        &self,
        calls: &[RuntimeToolActivity],
        width: u16,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        if let Some(call) = standalone_exploration_tool_call(calls) {
            return self.single_exploration_tool_call_lines_at(call, width, palette, marker_now);
        }

        let width = usize::from(width.max(1));
        let mut display_lines = exploration_display_lines(calls);
        coalesce_adjacent_target_display_lines(&mut display_lines);
        let mut lines = Vec::new();

        if !display_lines.is_empty() {
            let mut summary_lines = Vec::new();
            let group_family = exploration_group_family(calls);
            let active_started_at = self.active_marker_started_at.filter(|_| {
                calls.iter().any(|call| {
                    call.status != RuntimeToolActivityStatus::Failed
                        && matches!(
                            call.status,
                            RuntimeToolActivityStatus::Pending
                                | RuntimeToolActivityStatus::InProgress
                        )
                })
            });
            let marker_visible = active_started_at
                .map(|started_at| active_marker_visible_at(started_at, marker_now))
                .unwrap_or(true);
            let marker_color = if active_started_at.is_some() || self.exploration_open {
                palette.main
            } else {
                palette.quote
            };
            let marker_style = style_for_color(marker_color).add_modifier(Modifier::BOLD);
            let title = grouped_exploration_title(group_family, active_started_at.is_some());
            let marker_text = if marker_visible {
                TOOL_EXPLORATION_PREFIX
            } else {
                TOOL_RESULT_CONTINUATION_PREFIX
            };
            summary_lines.push(Line::from(vec![
                Span::styled(marker_text, marker_style),
                Span::styled(title, Style::new().add_modifier(Modifier::BOLD)),
            ]));

            for (index, display_line) in display_lines.iter().enumerate() {
                let line_prefix = if index == 0 {
                    TOOL_EXPLORATION_BRANCH_PREFIX
                } else {
                    TOOL_EXPLORATION_CHILD_PREFIX
                };
                summary_lines.extend(wrap_exploration_display_line(
                    display_line,
                    line_prefix,
                    TOOL_EXPLORATION_CHILD_PREFIX,
                    width,
                    palette,
                ));
            }
            append_message_block(&mut lines, summary_lines);
        }

        for call in calls
            .iter()
            .filter(|call| call.status == RuntimeToolActivityStatus::Failed)
        {
            append_message_block(
                &mut lines,
                self.failed_exploration_tool_call_lines_at(call, width, palette, marker_now),
            );
        }

        lines
    }

    fn single_exploration_tool_call_lines_at(
        &self,
        call: &RuntimeToolActivity,
        width: u16,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        // exploration 分组只承载不含 Diff content 的 call，使用空表避免占用主 item 的 cache。
        let diff_presentations = DiffPresentations::default();
        self.runtime_tool_activity_header_lines_at(
            call,
            &diff_presentations,
            width,
            palette,
            marker_now,
            self.exploration_open,
        )
    }

    fn failed_exploration_tool_call_lines_at(
        &self,
        call: &RuntimeToolActivity,
        width: usize,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        let diff_presentations = DiffPresentations::default();
        let mut lines = self.runtime_tool_activity_header_lines_at(
            call,
            &diff_presentations,
            width,
            palette,
            marker_now,
            false,
        );
        lines.extend(wrap_failed_exploration_detail_line(
            &failed_tool_call_detail_text(call),
            width,
            palette,
        ));
        lines
    }

    fn wrap_logical_line(
        &self,
        content_line: &str,
        logical_line: usize,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let initial_prefix = if logical_line == 0 {
            TOOL_RESULT_PREFIX
        } else {
            TOOL_RESULT_CONTINUATION_PREFIX
        };
        let prefix_width = display_width(initial_prefix);
        let content_width = width.saturating_sub(prefix_width).max(1);
        let logical_lines = self.wrap_content_line(content_line, content_width, palette);

        if logical_lines.is_empty() {
            return vec![Line::from(vec![self.prefix_span(initial_prefix, palette)])];
        }

        logical_lines
            .into_iter()
            .enumerate()
            .map(|(wrapped_index, content_spans)| {
                let prefix = if wrapped_index == 0 {
                    initial_prefix
                } else {
                    TOOL_RESULT_CONTINUATION_PREFIX
                };
                let mut spans = Vec::with_capacity(content_spans.len() + 1);
                spans.push(self.prefix_span(prefix, palette));
                spans.extend(content_spans);
                Line::from(spans)
            })
            .collect()
    }

    fn wrap_content_line(
        &self,
        content_line: &str,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Vec<Span<'static>>> {
        let Some(parsed) = ParsedToolResultLine::parse(content_line) else {
            return self.wrap_plain_content(content_line, width);
        };

        if !parsed.should_highlight_as_shell {
            return self.wrap_plain_result_content(&parsed.non_shell_display_text(), width);
        }

        self.wrap_shell_result_content(parsed, width, palette)
    }

    fn wrap_plain_content(&self, content_line: &str, width: usize) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| vec![Span::raw(line.text)])
            .collect()
    }

    fn wrap_plain_result_content(
        &self,
        content_line: &str,
        width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| style_core_result_line(line.text))
            .collect()
    }

    fn wrap_shell_result_content(
        &self,
        parsed: ParsedToolResultLine<'_>,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Vec<Span<'static>>> {
        let mut chunks = vec![HighlightChunk {
            text: parsed.verb.to_string(),
            style: Style::new().add_modifier(Modifier::BOLD),
        }];

        if !parsed.body.is_empty() {
            chunks.push(HighlightChunk {
                text: " ".to_string(),
                style: Style::new(),
            });
            chunks.extend(self.shell_command_chunks(parsed.body, palette));
        }

        wrap_highlight_chunks(&[chunks], width)
    }

    fn shell_command_chunks(&self, command: &str, palette: TerminalPalette) -> Vec<HighlightChunk> {
        self.shell_command_chunks_with_style(command, Style::new(), palette)
    }

    fn shell_command_chunks_with_style(
        &self,
        command: &str,
        base_style: Style,
        palette: TerminalPalette,
    ) -> Vec<HighlightChunk> {
        highlight_code_chunks(command, "bash", base_style, palette)
            .map(|highlighted| highlighted.into_iter().flatten().collect::<Vec<_>>())
            .filter(|chunks| !chunks.is_empty())
            .unwrap_or_else(|| {
                vec![HighlightChunk {
                    text: command.to_string(),
                    style: base_style,
                }]
            })
    }

    fn prefix_span(&self, prefix: &'static str, palette: TerminalPalette) -> Span<'static> {
        Span::styled(prefix, self.result_style(palette))
    }

    fn runtime_tool_activity_styled_lines_at(
        &self,
        call: &RuntimeToolActivity,
        width: u16,
        palette: TerminalPalette,
        marker_now: Instant,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        // presentation 按 content revision 懒构建一次；瞬态 marker 重绘只复用稳定结果。
        let empty_diff_presentations = DiffPresentations::default();
        let diff_presentations = self
            .diff_presentation_cache
            .get_or_build(call)
            .unwrap_or(&empty_diff_presentations);
        let mut lines = if self.should_use_detailed_execute_transcript(call) {
            Vec::new()
        } else {
            self.runtime_tool_activity_header_lines_at(
                call,
                diff_presentations,
                width,
                palette,
                marker_now,
                false,
            )
        };
        for block in runtime_tool_activity_detail_blocks(
            call,
            diff_presentations,
            self.render_mode,
            self.permission_waiting,
            &self.terminal_snapshots,
        ) {
            lines.extend(self.wrap_runtime_detail_block(&block, width, palette));
        }
        lines
    }

    fn should_use_detailed_execute_transcript(&self, call: &RuntimeToolActivity) -> bool {
        if !matches!(
            self.render_mode,
            ToolActivityRenderMode::Detailed | ToolActivityRenderMode::DebugDetailed
        ) || !is_execute_like_tool_call(call)
        {
            return false;
        }

        call.raw_output
            .as_ref()
            .and_then(|raw| raw.display_text())
            .is_some()
            || call
                .content
                .iter()
                .any(|content| matches!(content, RuntimeToolActivityContent::Terminal { .. }))
    }

    fn runtime_tool_activity_header_lines_at(
        &self,
        call: &RuntimeToolActivity,
        diff_presentations: &DiffPresentations,
        width: usize,
        palette: TerminalPalette,
        now: Instant,
        use_open_marker_color: bool,
    ) -> Vec<Line<'static>> {
        let active_started_at = self.active_marker_started_at.filter(|_| {
            matches!(
                call.status,
                RuntimeToolActivityStatus::Pending | RuntimeToolActivityStatus::InProgress
            )
        });
        let marker_visible = active_started_at
            .map(|started_at| active_marker_visible_at(started_at, now))
            .unwrap_or(true);
        let marker_text = if marker_visible {
            TOOL_RESULT_PREFIX
        } else {
            TOOL_RESULT_CONTINUATION_PREFIX
        };
        let marker_color = if active_started_at.is_some() || use_open_marker_color {
            palette.main
        } else {
            runtime_tool_activity_status_color(call.status, palette)
        };
        let status_style = style_for_color(marker_color).add_modifier(Modifier::BOLD);
        let location_style = style_for_color(palette.tertiary);
        let mut chunks = vec![HighlightChunk {
            text: marker_text.to_string(),
            style: status_style,
        }];
        chunks.extend(self.runtime_tool_activity_title_chunks(call, diff_presentations, palette));

        if !is_runtime_read_tool_activity(call)
            && !is_list_dir_tool_call(call)
            && !runtime_tool_activity_has_diff_content(call)
            && !is_runtime_write_tool_activity(call)
            && let Some(locations) = runtime_tool_activity_location_suffix(&call.locations)
        {
            chunks.push(HighlightChunk {
                text: format!(" {locations}"),
                style: location_style,
            });
        }

        wrap_highlight_chunks_soft(&[chunks], width)
            .into_iter()
            .map(Line::from)
            .collect()
    }

    fn runtime_tool_activity_title_chunks(
        &self,
        call: &RuntimeToolActivity,
        diff_presentations: &DiffPresentations,
        palette: TerminalPalette,
    ) -> Vec<HighlightChunk> {
        if let Some(chunks) =
            runtime_tool_activity_diff_header_chunks(call, diff_presentations, palette)
        {
            return chunks;
        }

        if is_runtime_read_tool_activity(call) {
            return runtime_read_tool_activity_title_chunks(call);
        }

        if is_runtime_write_tool_activity(call) {
            return runtime_write_tool_activity_title_chunks(call);
        }

        if is_list_dir_tool_call(call) {
            return list_dir_tool_call_title_chunks(call);
        }

        if let Some(chunks) =
            super::activity::specific_search_tool_activity_title_chunks(call, palette)
        {
            return chunks;
        }

        let title = runtime_tool_activity_display_title(call);
        let title_style = Style::new().add_modifier(Modifier::BOLD);
        if is_finished_execute_like_tool_call(call) {
            let title = execute_tool_call_shell_command(call)
                .unwrap_or_else(|| runtime_finished_execute_title(&title));
            let mut chunks = vec![
                HighlightChunk {
                    text: "Ran".to_string(),
                    style: title_style,
                },
                HighlightChunk {
                    text: " ".to_string(),
                    style: Style::new(),
                },
            ];
            if looks_like_shell_command(&title) {
                chunks.extend(self.shell_command_chunks(&title, palette));
            } else {
                chunks.push(HighlightChunk {
                    text: title,
                    style: Style::new(),
                });
            }
            return chunks;
        }

        if let Some(command) = execute_tool_call_shell_command(call) {
            if looks_like_shell_command(&command) {
                return self.shell_command_chunks_with_style(&command, title_style, palette);
            }

            return vec![HighlightChunk {
                text: command,
                style: title_style,
            }];
        }

        if looks_like_shell_command(&title) {
            return self.shell_command_chunks_with_style(&title, title_style, palette);
        }

        vec![HighlightChunk {
            text: title,
            style: title_style,
        }]
    }

    fn wrap_runtime_detail_block(
        &self,
        block: &RuntimeToolActivityDetailBlock,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        match block {
            RuntimeToolActivityDetailBlock::Text(logical_lines) => {
                self.wrap_runtime_text_detail_block(logical_lines, width, palette)
            }
            RuntimeToolActivityDetailBlock::SecondaryText(logical_lines) => {
                self.wrap_runtime_secondary_text_detail_block(logical_lines, width, palette)
            }
            RuntimeToolActivityDetailBlock::ExecuteTranscript(transcript) => {
                self.wrap_runtime_execute_transcript(transcript, width, palette)
            }
            RuntimeToolActivityDetailBlock::ExecuteFooter(footer) => {
                self.wrap_runtime_execute_footer(footer, width, palette)
            }
            RuntimeToolActivityDetailBlock::Diff(logical_lines) => {
                if effective_diff_display(self.diff_display, self.render_mode)
                    == DiffDisplay::Summary
                {
                    Vec::new()
                } else {
                    self.wrap_runtime_diff_detail_block(logical_lines, width, palette)
                }
            }
        }
    }

    fn wrap_runtime_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let prefix_style = style_for_color(palette.tertiary);
        logical_lines
            .iter()
            .enumerate()
            .flat_map(|(logical_index, content)| {
                let initial_prefix = if logical_index == 0 {
                    TOOL_ACTIVITY_DETAIL_PREFIX
                } else {
                    TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                };
                let prefix_width = display_width(initial_prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                let wrapped = wrap_prompt_visual_lines(content, content_width, 0);

                if wrapped.is_empty() {
                    return vec![Line::from(vec![Span::styled(initial_prefix, prefix_style)])];
                }

                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, line)| {
                        let prefix = if wrapped_index == 0 {
                            initial_prefix
                        } else {
                            TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                        };
                        Line::from(vec![
                            Span::styled(prefix, prefix_style),
                            Span::raw(line.text),
                        ])
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn wrap_runtime_secondary_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        self.wrap_runtime_styled_text_detail_block(
            logical_lines,
            width,
            palette,
            secondary_text_style(palette),
        )
    }

    fn wrap_runtime_execute_transcript(
        &self,
        transcript: &RuntimeExecuteTranscriptBlock,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let mut lines = self.wrap_runtime_execute_command_line(&transcript.command, width, palette);
        for output_line in &transcript.output_lines {
            lines.extend(self.wrap_runtime_plain_transcript_line(output_line, width));
        }
        lines
    }

    fn wrap_runtime_execute_command_line(
        &self,
        command: &str,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let mut chunks = vec![HighlightChunk {
            text: "$ ".to_string(),
            style: Style::new(),
        }];
        chunks.extend(self.shell_command_chunks(command, palette));

        wrap_highlight_chunks(&[chunks], width.max(1))
            .into_iter()
            .map(Line::from)
            .collect()
    }

    fn wrap_runtime_plain_transcript_line(
        &self,
        content: &str,
        width: usize,
    ) -> Vec<Line<'static>> {
        let wrapped = wrap_prompt_visual_lines(content, width.max(1), 0);
        if wrapped.is_empty() {
            return vec![Line::from("")];
        }

        wrapped
            .into_iter()
            .map(|line| Line::from(Span::raw(line.text)))
            .collect()
    }

    fn wrap_runtime_execute_footer(
        &self,
        footer: &RuntimeExecuteFooterLine,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let marker_color = match footer.status {
            RuntimeExecuteFooterStatus::Success => palette.quote,
            RuntimeExecuteFooterStatus::Failed => palette.system_error,
        };
        let chunks = vec![
            HighlightChunk {
                text: footer.marker.to_string(),
                style: style_for_color(marker_color).add_modifier(Modifier::BOLD),
            },
            HighlightChunk {
                text: footer.suffix.clone(),
                style: secondary_text_style(palette),
            },
        ];
        let mut lines = vec![Line::from("")];
        lines.extend(
            wrap_highlight_chunks(&[chunks], width.max(1))
                .into_iter()
                .map(Line::from),
        );
        lines
    }

    fn wrap_runtime_styled_text_detail_block(
        &self,
        logical_lines: &[String],
        width: usize,
        palette: TerminalPalette,
        content_style: Style,
    ) -> Vec<Line<'static>> {
        let prefix_style = style_for_color(palette.tertiary);
        logical_lines
            .iter()
            .enumerate()
            .flat_map(|(logical_index, content)| {
                let initial_prefix = if logical_index == 0 {
                    TOOL_ACTIVITY_DETAIL_PREFIX
                } else {
                    TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                };
                let prefix_width = display_width(initial_prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                let wrapped = wrap_highlight_chunks_soft(
                    &[vec![HighlightChunk {
                        text: content.clone(),
                        style: content_style,
                    }]],
                    content_width,
                );

                if wrapped.is_empty() {
                    return vec![Line::from(vec![Span::styled(initial_prefix, prefix_style)])];
                }

                wrapped
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, content_spans)| {
                        let prefix = if wrapped_index == 0 {
                            initial_prefix
                        } else {
                            TOOL_ACTIVITY_DETAIL_CONTINUATION_PREFIX
                        };
                        let mut spans = Vec::with_capacity(content_spans.len() + 1);
                        spans.push(Span::styled(prefix, prefix_style));
                        spans.extend(content_spans);
                        Line::from(spans)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn wrap_runtime_diff_detail_block(
        &self,
        logical_lines: &[RuntimeDiffDetailLine],
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let display = effective_diff_display(self.diff_display, self.render_mode);
        logical_lines
            .iter()
            .flat_map(|content| {
                let prefix = runtime_diff_line_prefix(content.line_number, content.kind);
                let continuation_prefix = " ".repeat(display_width(prefix.as_str()));
                let prefix_width = display_width(prefix.as_str());
                let content_width = width.saturating_sub(prefix_width).max(1);
                let gutter_style =
                    runtime_tool_activity_diff_gutter_style(content.kind, palette, display);
                let chunks = content
                    .segments
                    .iter()
                    .map(|segment| HighlightChunk {
                        text: segment.text.clone(),
                        style: runtime_tool_activity_diff_segment_style(
                            content.kind,
                            palette,
                            display,
                            segment.is_emphasized,
                        ),
                    })
                    .collect::<Vec<_>>();
                // `wrap_highlight_chunks` 对每个逻辑行至少产出一行（空正文即空 span 列表），
                // 因此这里不需要空结果分支：空行会自然渲染成只有 gutter 的一行。
                wrap_highlight_chunks(&[chunks], content_width)
                    .into_iter()
                    .enumerate()
                    .map(|(wrapped_index, content_spans)| {
                        let line_prefix = if wrapped_index == 0 {
                            prefix.clone()
                        } else {
                            continuation_prefix.clone()
                        };
                        let mut spans = Vec::with_capacity(content_spans.len() + 1);
                        spans.push(Span::styled(line_prefix, gutter_style));
                        spans.extend(content_spans);
                        let mut rendered = Line::from(spans);
                        rendered.style =
                            rendered
                                .style
                                .patch(runtime_tool_activity_diff_line_fill_style(
                                    content.kind,
                                    palette,
                                    display,
                                ));
                        rendered
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn result_style(&self, palette: TerminalPalette) -> Style {
        let color = match &self.body {
            ToolResultBody::Approval {
                kind: ToolResultKind::Ran,
                ..
            } => palette.quote,
            ToolResultBody::Approval {
                kind: ToolResultKind::Rejected,
                ..
            } => palette.approval_rejected,
            ToolResultBody::RuntimeToolActivity(call) => {
                runtime_tool_activity_status_color(call.status, palette)
            }
            ToolResultBody::Exploration(calls) => {
                if self.exploration_open {
                    palette.main
                } else if calls
                    .iter()
                    .any(|call| call.status == RuntimeToolActivityStatus::Failed)
                {
                    palette.approval_rejected
                } else {
                    palette.quote
                }
            }
        };

        if color == Color::Reset {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }

    fn refresh_render_cache_key(&mut self) {
        self.render_cache_key = tool_result_render_cache_key(
            &self.body,
            self.render_mode,
            self.exploration_open,
            self.approval_suspended,
            self.permission_waiting,
            &self.terminal_snapshots,
            self.diff_display,
        );
    }

    fn is_compact_approval_suspended(&self) -> bool {
        self.approval_suspended && self.render_mode == ToolActivityRenderMode::Compact
    }
}

fn append_message_block(
    lines: &mut Vec<Line<'static>>,
    block: impl IntoIterator<Item = Line<'static>>,
) {
    let mut block = block.into_iter().peekable();
    if block.peek().is_none() {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::raw(""));
    }
    lines.extend(block);
}

fn grouped_exploration_title(
    family: Option<ToolActivityGroupFamily>,
    is_active: bool,
) -> &'static str {
    match family {
        Some(ToolActivityGroupFamily::SkillUsage) => "Use Skills",
        Some(ToolActivityGroupFamily::Exploration) | None => {
            if is_active {
                "Exploring"
            } else {
                "Explored"
            }
        }
    }
}
