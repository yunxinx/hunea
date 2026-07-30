//! 工具活动 diff 的数据模型、计算预算、presentation 与渲染样式。

use std::{
    mem,
    rc::Rc,
    time::{Duration, Instant},
};

use ratatui::style::{Color, Modifier, Style};
use similar::{Algorithm, ChangeTag, DiffOp, DiffTag, DiffableStr, InlineChangeOptions, TextDiff};

use crate::{
    theme::{TerminalPalette, diff_emphasis_tint, diff_row_tint},
    transcript::TRANSCRIPT_DETAIL_HINT,
};
use runtime_domain::session::{RuntimeToolActivity, RuntimeToolActivityContent};

use super::{TOOL_ACTIVITY_COMPACT_EDGE_LINES, ToolActivityRenderMode};

/// diff gutter 为行号预留的固定宽度；更长行号自然向左扩展。
pub(super) const TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH: usize = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDiffDetailLine {
    pub(super) line_number: Option<usize>,
    pub(super) segments: Vec<RuntimeDiffSegment>,
    pub(super) kind: RuntimeDiffDetailLineKind,
}

/// `RuntimeDiffSegment` 是 diff 行内的一个连续文本片段；
/// `is_emphasized` 标记该片段属于行内实际变化区域，渲染时叠加强调样式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDiffSegment {
    pub(super) text: String,
    pub(super) is_emphasized: bool,
}

impl RuntimeDiffDetailLine {
    /// `plain` 构造不带行内强调的单段 diff 行。
    pub(super) fn plain(
        line_number: Option<usize>,
        text: String,
        kind: RuntimeDiffDetailLineKind,
    ) -> Self {
        Self {
            line_number,
            segments: vec![RuntimeDiffSegment {
                text,
                is_emphasized: false,
            }],
            kind,
        }
    }

    /// `joined_text` 返回全段拼接后的整行文本，仅用于测试断言。
    #[cfg(test)]
    pub(super) fn joined_text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeDiffDetailLineKind {
    Context,
    Insert,
    Delete,
    Separator,
    Omitted,
}

/// `RuntimeDiffPresentation` 是单个 Diff content 一次构建的共享产物：
/// header 的 +N/−N 计数与 detail 展示行同源，保证预算耗尽截断时两者按构造一致，
/// 也让同一 content 在单次工具结果构建中只跑一遍 diff。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RuntimeDiffPresentation {
    pub(super) detailed_lines: Rc<[RuntimeDiffDetailLine]>,
    compact_lines: Rc<[RuntimeDiffDetailLine]>,
    pub(super) added: usize,
    pub(super) removed: usize,
}

impl RuntimeDiffPresentation {
    pub(super) fn lines_for_render_mode(
        &self,
        render_mode: ToolActivityRenderMode,
    ) -> Rc<[RuntimeDiffDetailLine]> {
        if matches!(
            render_mode,
            ToolActivityRenderMode::Detailed | ToolActivityRenderMode::DebugDetailed
        ) {
            Rc::clone(&self.detailed_lines)
        } else {
            Rc::clone(&self.compact_lines)
        }
    }
}

/// `DiffPresentations` 持有一次工具活动中全部 Diff content 的共享 presentation。
///
/// 槽位与 `RuntimeToolActivity::content` 按下标对位、非 Diff content 为 `None`，
/// 该对位关系由唯一构造入口 `build` 直接映射 content 保证。
/// 无 Diff content 时槽位保持空表：按下标读写一律得到 `None`，
/// 省去 read/search/execute 等多数工具每次构建的无谓分配。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct DiffPresentations {
    slots: Vec<Option<Rc<RuntimeDiffPresentation>>>,
}

/// `DiffBudget` 为一次工具结果构建提供共享的 cooperative algorithm deadline：
/// deadline 贯穿行级 diff 与全部行内细化，耗尽后算法近似收敛，剩余 Replace op 直接走 plain。
/// `similar` 的 tokenization 与 lookup 构建不可中断，因此该值不是完整 presentation 的 hard deadline。
///
/// 以 newtype 承载而非裸 `Instant`，是为了在参数位置上无法与同类型的 marker 时间互换。
/// 行内调用必须用 `iter_inline_changes_with_options_deadline` 传入本 deadline——
/// 非 `_deadline` 变体每次调用都会自建独立的 500ms deadline，不继承 TextDiff 配置，
/// 会让总耗时随 Replace op 数量无界增长。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DiffBudget {
    deadline: Option<Instant>,
}

/// 每个 diff hunk 前后保留的 context 行数。
const DIFF_CONTEXT_LINES: usize = 3;
/// 每个 Diff content revision 共享的 diff algorithm 预算。
const DIFF_PRESENTATION_BUDGET_MS: u64 = 200;
/// Replace op 两侧行数之和超过该上限时跳过行内细化，回退整行样式。
/// 64 是现有边界测试与 benchmark 覆盖的最大可证明范围；扩大前必须补齐最坏输入基线。
const INLINE_MAX_OP_LINES: usize = 64;
/// Replace op 内任一行字符数超过该上限时整个 op 回退整行样式。
const INLINE_MAX_LINE_CHARS: usize = 1000;

impl DiffBudget {
    /// `for_presentation` 从实际 cache miss 开始计算一次性预算。
    pub(super) fn for_presentation() -> Self {
        Self {
            deadline: Some(Instant::now() + Duration::from_millis(DIFF_PRESENTATION_BUDGET_MS)),
        }
    }

    /// 正确性测试不受线程调度影响；deadline 行为由 `exhausted` 路径单独验证。
    #[cfg(test)]
    pub(super) fn unlimited_for_test() -> Self {
        Self { deadline: None }
    }

    /// `exhausted` 构造一个立即耗尽的预算，用于测试全回退路径。
    #[cfg(test)]
    pub(super) fn exhausted() -> Self {
        Self {
            deadline: Some(Instant::now()),
        }
    }

    fn deadline(self) -> Option<Instant> {
        self.deadline
    }

    /// `is_exhausted` 判断预算是否已用尽；用尽后跳过后续行内细化。
    fn is_exhausted(self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }
}

/// `diff_detail_line_views` 在 presentation 构建期一次生成 detailed/compact 两种稳定视图。
/// 通知参与 compact 截断计算；compact 只复制固定数量的边缘行，逐帧渲染只 clone `Rc`。
fn diff_detail_line_views(
    mut lines: Vec<RuntimeDiffDetailLine>,
    is_preview_truncated: bool,
) -> (Rc<[RuntimeDiffDetailLine]>, Rc<[RuntimeDiffDetailLine]>) {
    if is_preview_truncated {
        lines.insert(0, preview_truncation_notice_line());
    }

    let detailed_lines: Rc<[RuntimeDiffDetailLine]> = lines.into();
    let edge = TOOL_ACTIVITY_COMPACT_EDGE_LINES;
    let limit = edge.saturating_mul(2);
    if detailed_lines.len() <= limit {
        return (Rc::clone(&detailed_lines), detailed_lines);
    }

    let omitted = detailed_lines.len().saturating_sub(limit);
    let mut compact_lines = Vec::with_capacity(limit + 1);
    compact_lines.extend(detailed_lines.iter().take(edge).cloned());
    compact_lines.push(RuntimeDiffDetailLine::plain(
        None,
        format!("⋮ +{omitted} lines ({TRANSCRIPT_DETAIL_HINT})"),
        RuntimeDiffDetailLineKind::Omitted,
    ));
    compact_lines.extend(
        detailed_lines
            .iter()
            .skip(detailed_lines.len().saturating_sub(edge))
            .cloned(),
    );
    (detailed_lines, compact_lines.into())
}

/// 预览在上游被截断时前插的提示行。
fn preview_truncation_notice_line() -> RuntimeDiffDetailLine {
    RuntimeDiffDetailLine::plain(
        None,
        "⋮ preview truncated; showing partial diff".to_string(),
        RuntimeDiffDetailLineKind::Omitted,
    )
}

/// 无预算参数的便捷包装，仅供测试验证 diff 行级行为。
#[cfg(test)]
pub(super) fn diff_detail_lines(
    old_text: Option<&str>,
    new_text: &str,
) -> Vec<RuntimeDiffDetailLine> {
    diff_detail_lines_with_budget(old_text, new_text, DiffBudget::unlimited_for_test())
}

/// `diff_detail_lines_with_budget` 是 diff 计算的可注入入口：行级 diff 与全部行内细化
/// 共享同一份预算。测试可注入已耗尽预算断言全 plain 回退且不 panic。
pub(super) fn diff_detail_lines_with_budget(
    old_text: Option<&str>,
    new_text: &str,
    budget: DiffBudget,
) -> Vec<RuntimeDiffDetailLine> {
    let Some(old_text) = old_text else {
        // 空文本没有行 token；不制造占位 Insert，确保 detail 与 header 计数同源。
        return new_text
            .tokenize_lines()
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                RuntimeDiffDetailLine::plain(
                    Some(index + 1),
                    strip_line_terminator(line).to_string(),
                    RuntimeDiffDetailLineKind::Insert,
                )
            })
            .collect();
    };

    let diff = build_line_diff(old_text, new_text, budget);
    let inline_options = deadline_controlled_inline_options();

    let mut lines = Vec::new();
    for (group_index, group) in diff_presentation_groups(diff.ops(), DIFF_CONTEXT_LINES)
        .iter()
        .enumerate()
    {
        if group_index > 0 {
            lines.push(RuntimeDiffDetailLine::plain(
                None,
                "⋮".to_string(),
                RuntimeDiffDetailLineKind::Separator,
            ));
        }

        for op in group {
            // 预算耗尽后剩余 Replace op 直接走 plain，连 tokenize 都跳过。
            if op.tag() == DiffTag::Replace
                && !budget.is_exhausted()
                && replace_op_within_inline_budget(&diff, op)
            {
                for change in diff.iter_inline_changes_with_options_deadline(
                    op,
                    inline_options,
                    budget.deadline(),
                ) {
                    let kind = diff_line_kind(change.tag());
                    lines.push(RuntimeDiffDetailLine {
                        line_number: diff_line_number(kind, change.old_index(), change.new_index()),
                        segments: inline_change_segments(&change),
                        kind,
                    });
                }
            } else {
                // 纯 Insert/Delete 块、Context、预算外 Replace 与预算耗尽后走整行样式。
                for change in diff.iter_changes(op) {
                    let kind = diff_line_kind(change.tag());
                    lines.push(RuntimeDiffDetailLine::plain(
                        diff_line_number(kind, change.old_index(), change.new_index()),
                        strip_line_terminator(change.value()).to_string(),
                        kind,
                    ));
                }
            }
        }
    }

    lines
}

/// 按 TUI presentation 语义将 diff op 划分为 hunk，并裁剪首尾 context。
///
/// 两个各含 `context_lines` 行的 context 窗口仅相接时仍保留视觉分隔；
/// 只有窗口真正重叠（Equal gap 小于两倍 context）才合并为同一 hunk。
fn diff_presentation_groups(ops: &[DiffOp], context_lines: usize) -> Vec<Vec<DiffOp>> {
    let Some(first_change) = ops.iter().position(|op| op.tag() != DiffTag::Equal) else {
        return Vec::new();
    };
    let last_change = ops
        .iter()
        .rposition(|op| op.tag() != DiffTag::Equal)
        .expect("first_change guarantees a final change");

    let mut groups = Vec::new();
    let mut current_group = Vec::new();

    if let Some(DiffOp::Equal {
        old_index,
        new_index,
        len,
    }) = first_change
        .checked_sub(1)
        .and_then(|index| ops.get(index))
        .copied()
    {
        let kept_len = len.min(context_lines);
        if kept_len > 0 {
            let offset = len - kept_len;
            current_group.push(DiffOp::Equal {
                old_index: old_index + offset,
                new_index: new_index + offset,
                len: kept_len,
            });
        }
    }

    let split_gap_lines = context_lines.saturating_mul(2);
    for op in ops[first_change..=last_change].iter().copied() {
        if let DiffOp::Equal {
            old_index,
            new_index,
            len,
        } = op
            && len >= split_gap_lines
        {
            if context_lines > 0 {
                current_group.push(DiffOp::Equal {
                    old_index,
                    new_index,
                    len: context_lines,
                });
            }
            groups.push(mem::take(&mut current_group));

            if context_lines > 0 {
                let offset = len - context_lines;
                current_group.push(DiffOp::Equal {
                    old_index: old_index + offset,
                    new_index: new_index + offset,
                    len: context_lines,
                });
            }
        } else {
            current_group.push(op);
        }
    }

    if let Some(DiffOp::Equal {
        old_index,
        new_index,
        len,
    }) = ops.get(last_change + 1).copied()
    {
        let kept_len = len.min(context_lines);
        if kept_len > 0 {
            current_group.push(DiffOp::Equal {
                old_index,
                new_index,
                len: kept_len,
            });
        }
    }

    groups.push(current_group);
    groups
}

fn deadline_controlled_inline_options() -> InlineChangeOptions {
    // semantic cleanup 在 deadline-controlled algorithm 后运行且不可中断；render 路径保持默认关闭。
    InlineChangeOptions::new()
}

#[cfg(test)]
pub(super) fn inline_semantic_cleanup_enabled() -> bool {
    deadline_controlled_inline_options().semantic_cleanup_enabled()
}

/// `build_line_diff` 用统一的 Myers 算法构建行级 diff，并让行级计算与行内细化
/// 共享调用方传入的 deadline。
fn build_line_diff<'old, 'new>(
    old_text: &'old str,
    new_text: &'new str,
    budget: DiffBudget,
) -> TextDiff<'old, 'new, str> {
    let mut config = TextDiff::configure();
    config.algorithm(Algorithm::Myers);
    if let Some(deadline) = budget.deadline() {
        config.deadline(deadline);
    }
    config.diff_lines(old_text, new_text)
}

fn diff_line_kind(tag: ChangeTag) -> RuntimeDiffDetailLineKind {
    match tag {
        ChangeTag::Equal => RuntimeDiffDetailLineKind::Context,
        ChangeTag::Insert => RuntimeDiffDetailLineKind::Insert,
        ChangeTag::Delete => RuntimeDiffDetailLineKind::Delete,
    }
}

/// 行号规则与现状一致：Insert/Context 用新文件行号，Delete 用旧文件行号，均从 1 开始。
fn diff_line_number(
    kind: RuntimeDiffDetailLineKind,
    old_index: Option<usize>,
    new_index: Option<usize>,
) -> Option<usize> {
    let index = match kind {
        RuntimeDiffDetailLineKind::Delete => old_index,
        _ => new_index,
    };
    index.map(|index| index + 1)
}

/// `replace_op_within_inline_budget` 判断 Replace op 是否在行内细化预算内；
/// 超出预算的超大块或超长行回退整行样式，避免细化开销失控。
/// 行长以 strip 行终止符后的正文字符计，`\n`/`\r\n`/`\r` 不占预算。
fn replace_op_within_inline_budget(diff: &TextDiff<'_, '_, str>, op: &DiffOp) -> bool {
    let old_range = op.old_range();
    let new_range = op.new_range();
    if old_range.len().saturating_add(new_range.len()) > INLINE_MAX_OP_LINES {
        return false;
    }

    let line_within_budget = |line: Option<&str>| {
        line.is_some_and(|line| {
            strip_line_terminator(line)
                .chars()
                .take(INLINE_MAX_LINE_CHARS + 1)
                .count()
                <= INLINE_MAX_LINE_CHARS
        })
    };
    old_range
        .clone()
        .all(|index| line_within_budget(diff.old_slice(index)))
        && new_range
            .clone()
            .all(|index| line_within_budget(diff.new_slice(index)))
}

/// `inline_change_segments` 把一条行内变更展开为段序列，并从末段 strip 行终止符。
/// strip 后全段拼接必须等于原行文本（不含行尾换行）。
fn inline_change_segments(change: &similar::InlineChange<'_, str>) -> Vec<RuntimeDiffSegment> {
    let mut segments: Vec<RuntimeDiffSegment> = change
        .iter_strings_lossy()
        .map(|(is_emphasized, text)| RuntimeDiffSegment {
            text: text.into_owned(),
            is_emphasized,
        })
        .collect();

    if let Some(last) = segments.last_mut() {
        strip_line_terminator_in_place(&mut last.text);
    }
    // strip 后可能出现空末段（例如末段恰为换行符本身）；丢弃避免零宽 chunk。
    while segments
        .last()
        .is_some_and(|segment| segment.text.is_empty())
        && segments.len() > 1
    {
        segments.pop();
    }
    if segments.is_empty() {
        segments.push(RuntimeDiffSegment {
            text: String::new(),
            is_emphasized: false,
        });
    }
    segments
}

pub(super) fn strip_line_terminator_in_place(line: &mut String) {
    // 只裁掉 ASCII 行终止符，返回的长度必定位于 UTF-8 边界；truncate 会保留原 allocation。
    let body_len = strip_line_terminator(line).len();
    line.truncate(body_len);
}

/// `strip_line_terminator` 移除行尾终止符：`\n`、`\r\n` 或裸 `\r`。
/// 三种形态与 similar `tokenize_lines` 的行切分语义一一对应（裸 `\r` 也被其视为行终止符），
/// 每个行 token 至多携带一个终止符，因此单次 strip 即可还原不含终止符的整行文本。
fn strip_line_terminator(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

impl DiffPresentations {
    /// 为一个 content revision 构建 presentation；预算从实际 cache miss 开始，
    /// 不借用 marker/frame 时间，避免前序 frame 工作侵蚀 diff 的一次性预算。
    pub(super) fn build_for_content_revision(call: &RuntimeToolActivity) -> Self {
        Self::build(call, DiffBudget::for_presentation())
    }

    /// `build` 为一次工具活动构建全部 Diff content 的 presentation。
    /// 全部 content 共享同一个 cooperative algorithm deadline，避免每个文件
    /// 重新获得完整预算；tokenization 与输出物化的开销仍随输入规模增长。
    pub(super) fn build(call: &RuntimeToolActivity, budget: DiffBudget) -> Self {
        let has_diff_content = call
            .content
            .iter()
            .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }));
        if !has_diff_content {
            return Self::default();
        }

        let slots = call
            .content
            .iter()
            .map(|content| {
                let RuntimeToolActivityContent::Diff {
                    old_text,
                    new_text,
                    is_truncated,
                    ..
                } = content
                else {
                    return None;
                };
                Some(Rc::new(runtime_diff_presentation(
                    old_text.as_deref(),
                    new_text,
                    *is_truncated,
                    budget,
                )))
            })
            .collect();
        Self { slots }
    }

    /// `for_content` 借出第 `index` 个 content 的 presentation。
    pub(super) fn for_content(&self, index: usize) -> Option<&RuntimeDiffPresentation> {
        self.slots.get(index)?.as_deref()
    }

    pub(super) fn presentation_for_content(
        &self,
        index: usize,
    ) -> Option<Rc<RuntimeDiffPresentation>> {
        self.slots.get(index)?.as_ref().map(Rc::clone)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

fn runtime_diff_presentation(
    old_text: Option<&str>,
    new_text: &str,
    is_preview_truncated: bool,
    budget: DiffBudget,
) -> RuntimeDiffPresentation {
    let lines = diff_detail_lines_with_budget(old_text, new_text, budget);
    // hunk 分组只裁剪 Equal 上下文、从不省略变更行；统一从实际展示行统计，
    // 让所有输入路径在预算回退与空文本边界上都保持 header/detail 一致。
    let (added, removed) = lines
        .iter()
        .fold((0, 0), |(added, removed), line| match line.kind {
            RuntimeDiffDetailLineKind::Insert => (added + 1, removed),
            RuntimeDiffDetailLineKind::Delete => (added, removed + 1),
            RuntimeDiffDetailLineKind::Context
            | RuntimeDiffDetailLineKind::Separator
            | RuntimeDiffDetailLineKind::Omitted => (added, removed),
        });
    let (detailed_lines, compact_lines) = diff_detail_line_views(lines, is_preview_truncated);
    RuntimeDiffPresentation {
        detailed_lines,
        compact_lines,
        added,
        removed,
    }
}

/// 只比较会影响 presentation 的字段；path 由 header 直接读取，不触发昂贵 diff 重建。
/// `len + zip` 保留 content 槽位结构，避免 Diff 插入/删除后复用错位结果。
pub(super) fn has_same_diff_presentation_inputs(
    current: &[RuntimeToolActivityContent],
    next: &[RuntimeToolActivityContent],
) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(current, next)| match (current, next) {
                (
                    RuntimeToolActivityContent::Diff {
                        old_text: current_old,
                        new_text: current_new,
                        is_truncated: current_truncated,
                        ..
                    },
                    RuntimeToolActivityContent::Diff {
                        old_text: next_old,
                        new_text: next_new,
                        is_truncated: next_truncated,
                        ..
                    },
                ) => {
                    current_old == next_old
                        && current_new == next_new
                        && current_truncated == next_truncated
                }
                (RuntimeToolActivityContent::Diff { .. }, _)
                | (_, RuntimeToolActivityContent::Diff { .. }) => false,
                _ => true,
            })
}

pub(super) fn runtime_tool_activity_has_diff_content(call: &RuntimeToolActivity) -> bool {
    call.content
        .iter()
        .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
}

pub(super) fn runtime_diff_line_prefix(
    line_number: Option<usize>,
    kind: RuntimeDiffDetailLineKind,
) -> String {
    let sign = match kind {
        RuntimeDiffDetailLineKind::Insert => "+",
        RuntimeDiffDetailLineKind::Delete => "-",
        RuntimeDiffDetailLineKind::Context
        | RuntimeDiffDetailLineKind::Separator
        | RuntimeDiffDetailLineKind::Omitted => " ",
    };
    match line_number {
        Some(line_number) => format!(
            "{line_number:>width$} {sign}  ",
            width = TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH
        ),
        None => " ".repeat(TOOL_ACTIVITY_DIFF_LINE_NUMBER_WIDTH.saturating_sub(1)),
    }
}

pub(super) fn runtime_tool_activity_diff_line_style(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    match kind {
        RuntimeDiffDetailLineKind::Context => Style::new(),
        RuntimeDiffDetailLineKind::Insert => Style::new().fg(palette.quote),
        RuntimeDiffDetailLineKind::Delete => Style::new().fg(palette.system_error),
        RuntimeDiffDetailLineKind::Separator | RuntimeDiffDetailLineKind::Omitted => {
            Style::new().fg(palette.tertiary)
        }
    }
}

pub(super) fn runtime_tool_activity_diff_row_style(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    runtime_tool_activity_diff_background(kind, palette)
        .map(|background| Style::new().bg(background))
        .unwrap_or_default()
}

fn runtime_tool_activity_diff_background(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Option<Color> {
    match kind {
        RuntimeDiffDetailLineKind::Context => None,
        RuntimeDiffDetailLineKind::Insert => diff_row_tint(&palette, true),
        RuntimeDiffDetailLineKind::Delete => diff_row_tint(&palette, false),
        RuntimeDiffDetailLineKind::Separator | RuntimeDiffDetailLineKind::Omitted => None,
    }
}

/// `runtime_tool_activity_diff_emphasis_style` 返回行内强调段叠加在整行样式之上的样式。
/// emphasis tint 可用时用更饱和背景 + BOLD；终端默认配色下降级为 REVERSED 保证可辨识。
pub(super) fn runtime_tool_activity_diff_emphasis_style(
    kind: RuntimeDiffDetailLineKind,
    palette: TerminalPalette,
) -> Style {
    let is_insert = match kind {
        RuntimeDiffDetailLineKind::Insert => true,
        RuntimeDiffDetailLineKind::Delete => false,
        RuntimeDiffDetailLineKind::Context
        | RuntimeDiffDetailLineKind::Separator
        | RuntimeDiffDetailLineKind::Omitted => return Style::new(),
    };

    match diff_emphasis_tint(&palette, is_insert) {
        Some(tint) => Style::new().bg(tint).add_modifier(Modifier::BOLD),
        None => Style::new().add_modifier(Modifier::REVERSED),
    }
}
