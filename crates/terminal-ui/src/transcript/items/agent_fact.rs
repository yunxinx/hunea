use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use runtime_domain::agent::{AgentLaunchSnapshot, AgentOutcome, AgentOutcomeSnapshot};

use crate::{
    display_width::display_width,
    styled_text::{line_plain_text_len, lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style, tertiary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        markdown_highlight::{HighlightChunk, wrap_highlight_chunks_soft},
    },
};

/// 视觉语言对齐 `tool_result/item.rs` 的既有 marker/树形前缀；不跨模块扩张可见性，
/// 在本模块内保持相同的字符串值（`● `、`  └ `、`    `）。
const AGENT_FACT_MARKER: &str = "● ";
/// `├` 在主文档流中是新的同级连接符（entry_tree 已有 sibling 树先例）；宽度与 `  └ ` 一致。
const AGENT_FACT_BRANCH_PREFIX: &str = "  ├ ";
const AGENT_FACT_LAST_BRANCH_PREFIX: &str = "  └ ";
const AGENT_FACT_CONTINUATION_PREFIX: &str = "    ";

/// `AgentFactItem` 表示 parent document timeline 中一条 append-only 的 Agent 事实。
///
/// 照 `WorkDurationMessageItem` 的最小模式：snapshot 在构造时冻结、cache key 一次计算、
/// 不提供任何 mutator/lookup——后续 status/permission/metrics delta 只能追加新 item，
/// 永远不能回写已落文档的 launch/outcome 事实。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFactItem {
    fact: AgentFactKind,
    render_cache_key: u64,
}

/// launch 与 outcome 是 parent document timeline 中仅有的两类 Agent 事实；
/// 单一 `TranscriptItem` variant 的内层区分。
#[derive(Debug, Clone, PartialEq, Eq)]
enum AgentFactKind {
    /// 一个 snapshot = 一次显式 typed batch launch；group 边界严格等于 snapshot 边界。
    Launch(AgentLaunchSnapshot),
    Outcome(AgentOutcomeSnapshot),
}

impl AgentFactItem {
    /// `launch` 冻结一次 committed typed batch launch 的 durable fact。
    pub fn launch(snapshot: AgentLaunchSnapshot) -> Self {
        let render_cache_key = agent_launch_fact_render_cache_key(&snapshot);
        Self {
            fact: AgentFactKind::Launch(snapshot),
            render_cache_key,
        }
    }

    /// `outcome` 冻结一个 child Agent 的 terminal outcome fact。
    pub fn outcome(snapshot: AgentOutcomeSnapshot) -> Self {
        let render_cache_key = agent_outcome_fact_render_cache_key(&snapshot);
        Self {
            fact: AgentFactKind::Outcome(snapshot),
            render_cache_key,
        }
    }

    /// `render_lines` 渲染 launch group 或 terminal outcome 的语义文本。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        match &self.fact {
            AgentFactKind::Launch(snapshot) => launch_fact_lines(snapshot, width, palette),
            AgentFactKind::Outcome(snapshot) => outcome_fact_lines(snapshot, width, palette),
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
    pub fn render_for_terminal_replay(
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
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        lines_to_plain_text(&self.render_lines(width, palette))
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    /// Agent 事实是 TUI-only document item，不进入模型上下文。
    pub(crate) fn source_text_byte_len(&self) -> usize {
        0
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        let lines = self.render_lines(width, palette);
        let content_char_len = lines.iter().map(line_plain_text_len).sum();
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
}

/// 单 request launch 渲染为单行 `● Launched <title>`；group（children > 1）渲染
/// `● Launched N agents` header + 两空格缩进的 `├`/`└` 同级 title list。
/// 空 children 只可能来自畸形持久化数据，降级为 count header，不渲染 title 行。
fn launch_fact_lines(
    snapshot: &AgentLaunchSnapshot,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if snapshot.children.len() == 1 {
        let title = snapshot.children[0].title.as_str();
        return fact_header_lines("Launched", title, width, palette);
    }

    let mut lines = group_launch_header_lines(snapshot.children.len(), width, palette);
    for (index, child) in snapshot.children.iter().enumerate() {
        let is_last = index + 1 == snapshot.children.len();
        let line_prefix = if is_last {
            AGENT_FACT_LAST_BRANCH_PREFIX
        } else {
            AGENT_FACT_BRANCH_PREFIX
        };
        lines.extend(indented_fact_lines(
            child.title.as_str(),
            line_prefix,
            agent_fact_child_title_style(palette),
            width,
            palette,
        ));
    }
    lines
}

/// terminal outcome 渲染为 `● Completed|Failed|Cancelled <title>`，
/// 有 delivery-safe summary 时追加一行 `  └ ` 缩进的 secondary summary。
fn outcome_fact_lines(
    snapshot: &AgentOutcomeSnapshot,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let action = match snapshot.outcome {
        AgentOutcome::Completed => "Completed",
        AgentOutcome::Failed => "Failed",
        AgentOutcome::Cancelled => "Cancelled",
    };
    let mut lines = fact_header_lines(action, snapshot.title.as_str(), width, palette);
    if let Some(summary) = &snapshot.summary {
        lines.extend(indented_fact_lines(
            summary.as_str(),
            AGENT_FACT_LAST_BRANCH_PREFIX,
            secondary_text_style(palette),
            width,
            palette,
        ));
    }
    lines
}

/// header 行：marker（BOLD + settled 槽位）+ action（次级强调）+ title（主要扫描目标）。
fn fact_header_lines(
    action: &str,
    title: &str,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let chunks = vec![
        HighlightChunk {
            text: AGENT_FACT_MARKER.to_string(),
            style: agent_fact_marker_style(palette),
        },
        HighlightChunk {
            text: format!("{action} "),
            style: secondary_text_style(palette),
        },
        HighlightChunk {
            text: title.to_string(),
            style: agent_fact_title_style(palette),
        },
    ];
    wrapped_chunk_lines(&[chunks], width)
}

fn group_launch_header_lines(
    child_count: usize,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let chunks = vec![
        HighlightChunk {
            text: AGENT_FACT_MARKER.to_string(),
            style: agent_fact_marker_style(palette),
        },
        HighlightChunk {
            text: format!("Launched {child_count} agents"),
            style: secondary_text_style(palette),
        },
    ];
    wrapped_chunk_lines(&[chunks], width)
}

/// 子行/summary 行：首行带树形 prefix，wrap 续行对齐 `    `（同 exploration 子行节奏）。
fn indented_fact_lines(
    text: &str,
    line_prefix: &'static str,
    content_style: Style,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let prefix_width = display_width(line_prefix);
    let content_width = width.saturating_sub(prefix_width).max(1);
    let chunks = vec![HighlightChunk {
        text: text.to_string(),
        style: content_style,
    }];
    let wrapped = wrap_highlight_chunks_soft(&[chunks], content_width);
    if wrapped.is_empty() {
        return vec![Line::from(vec![Span::styled(
            line_prefix,
            tertiary_text_style(palette),
        )])];
    }

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, content_spans)| {
            let prefix = if index == 0 {
                line_prefix
            } else {
                AGENT_FACT_CONTINUATION_PREFIX
            };
            let mut spans = Vec::with_capacity(content_spans.len() + 1);
            spans.push(Span::styled(prefix, tertiary_text_style(palette)));
            spans.extend(content_spans);
            Line::from(spans)
        })
        .collect()
}

fn wrapped_chunk_lines(chunks: &[Vec<HighlightChunk>], width: usize) -> Vec<Line<'static>> {
    wrap_highlight_chunks_soft(chunks, width)
        .into_iter()
        .map(Line::from)
        .collect()
}

/// marker 复用 settled exploration marker 的 `palette.quote` 槽位：launch/outcome 都是
/// 已定局事实，与"活动进行中"的 `palette.main` 语义区分；失败语义由 action 词承载，
/// 不使用 `system_error` 红，避免与 error-styled SystemMessageItem 混淆。
fn agent_fact_marker_style(palette: TerminalPalette) -> Style {
    style_for_foreground(palette.quote).add_modifier(Modifier::BOLD)
}

/// title 是主要扫描目标：BOLD 复用 tool result title 的字体，颜色用 primary 槽位。
fn agent_fact_title_style(palette: TerminalPalette) -> Style {
    style_for_foreground(palette.main).add_modifier(Modifier::BOLD)
}

/// group 子行 title 用 primary 色（不 BOLD——BOLD 留给 header 行的单一扫描锚点）。
fn agent_fact_child_title_style(palette: TerminalPalette) -> Style {
    style_for_foreground(palette.main)
}

/// 与 `tool_result::activity::style_for_color` 同语义的本地实现：`Color::Reset` 依赖
/// 终端默认前景（terminal_default_palette 场景），不显式设置 fg。
fn style_for_foreground(color: Color) -> Style {
    if color == Color::Reset {
        Style::new()
    } else {
        Style::new().fg(color)
    }
}

/// cache key 覆盖 snapshot 全部语义字段：内容不同的 fact 不得共享渲染缓存。
fn agent_launch_fact_render_cache_key(snapshot: &AgentLaunchSnapshot) -> u64 {
    let mut hasher = DefaultHasher::new();
    "agent_fact_launch".hash(&mut hasher);
    snapshot.group_id.hash(&mut hasher);
    snapshot.parent_agent_id.hash(&mut hasher);
    snapshot.parent_turn_id.hash(&mut hasher);
    snapshot.occurred_at_ms.hash(&mut hasher);
    for child in &snapshot.children {
        child.agent_id.hash(&mut hasher);
        child.title.hash(&mut hasher);
        child.objective.hash(&mut hasher);
    }
    hasher.finish()
}

fn agent_outcome_fact_render_cache_key(snapshot: &AgentOutcomeSnapshot) -> u64 {
    let mut hasher = DefaultHasher::new();
    "agent_fact_outcome".hash(&mut hasher);
    snapshot.agent_id.hash(&mut hasher);
    snapshot.title.hash(&mut hasher);
    snapshot.group_id.hash(&mut hasher);
    snapshot.parent_agent_id.hash(&mut hasher);
    snapshot.parent_turn_id.hash(&mut hasher);
    snapshot.outcome.hash(&mut hasher);
    snapshot.occurred_at_ms.hash(&mut hasher);
    snapshot.summary.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        styled_text::line_to_plain_text,
        theme::{default_palette, secondary_text_style, tertiary_text_style},
    };
    use runtime_domain::agent::{
        AgentId, AgentLaunchChildSnapshot, AgentLaunchGroupId, AgentObjective,
        AgentObjectiveSummary, AgentOutcomeSummary, AgentTitle, AgentTurnId,
    };

    fn test_title(text: &str) -> AgentTitle {
        AgentTitle::resolve(
            &AgentObjective::new("fallback objective").expect("objective should be valid"),
            Some(text),
        )
        .expect("title should resolve")
    }

    fn test_objective(text: &str) -> AgentObjectiveSummary {
        AgentObjectiveSummary::from_objective(
            &AgentObjective::new(text).expect("objective should be valid"),
        )
        .expect("objective summary should resolve")
    }

    fn launch_snapshot(children: Vec<AgentLaunchChildSnapshot>) -> AgentLaunchSnapshot {
        AgentLaunchSnapshot {
            group_id: AgentLaunchGroupId::new(7),
            parent_agent_id: AgentId::MAIN,
            parent_turn_id: AgentTurnId::new(9),
            children,
            occurred_at_ms: 42,
        }
    }

    fn launch_child(agent_id: u64, title: &str) -> AgentLaunchChildSnapshot {
        AgentLaunchChildSnapshot {
            agent_id: AgentId::new(agent_id),
            title: test_title(title),
            objective: test_objective("objective body"),
        }
    }

    fn outcome_snapshot(outcome: AgentOutcome) -> AgentOutcomeSnapshot {
        AgentOutcomeSnapshot {
            agent_id: AgentId::new(2),
            title: test_title("research task"),
            group_id: Some(AgentLaunchGroupId::new(7)),
            parent_agent_id: Some(AgentId::MAIN),
            parent_turn_id: Some(AgentTurnId::new(9)),
            outcome,
            occurred_at_ms: 43,
            summary: Some(
                AgentOutcomeSummary::new("Child Agent completed").expect("summary should resolve"),
            ),
        }
    }

    #[test]
    fn single_launch_renders_one_marker_line() {
        let palette = default_palette();
        let item = AgentFactItem::launch(launch_snapshot(vec![launch_child(2, "research task")]));
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["● Launched research task".to_string()]
        );
        // marker BOLD + settled 槽位；action 次级；title primary + BOLD。
        assert_eq!(lines[0].spans[0].style, agent_fact_marker_style(palette));
        assert_eq!(lines[0].spans[1].style, secondary_text_style(palette));
        assert_eq!(lines[0].spans[2].style, agent_fact_title_style(palette));
    }

    #[test]
    fn group_launch_renders_header_and_branch_title_list() {
        let palette = default_palette();
        let item = AgentFactItem::launch(launch_snapshot(vec![
            launch_child(2, "first task"),
            launch_child(3, "second task"),
            launch_child(4, "third task"),
        ]));
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec![
                "● Launched 3 agents".to_string(),
                "  ├ first task".to_string(),
                "  ├ second task".to_string(),
                "  └ third task".to_string(),
            ]
        );
        assert_eq!(lines[1].spans[0].style, tertiary_text_style(palette));
        assert_eq!(
            lines[1].spans[1].style,
            agent_fact_child_title_style(palette)
        );
    }

    #[test]
    fn outcome_renders_three_states_with_delivery_safe_summary() {
        let palette = default_palette();
        for (outcome, action) in [
            (AgentOutcome::Completed, "Completed"),
            (AgentOutcome::Failed, "Failed"),
            (AgentOutcome::Cancelled, "Cancelled"),
        ] {
            let item = AgentFactItem::outcome(outcome_snapshot(outcome));
            let lines = item.render_lines(80, palette);

            assert_eq!(
                lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
                vec![
                    format!("● {action} research task"),
                    "  └ Child Agent completed".to_string(),
                ]
            );
            assert_eq!(lines[1].spans[0].style, tertiary_text_style(palette));
            assert_eq!(lines[1].spans[1].style, secondary_text_style(palette));
        }
    }

    #[test]
    fn outcome_without_summary_renders_single_line() {
        let palette = default_palette();
        let snapshot = AgentOutcomeSnapshot {
            summary: None,
            ..outcome_snapshot(AgentOutcome::Cancelled)
        };
        let lines = AgentFactItem::outcome(snapshot).render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["● Cancelled research task".to_string()]
        );
    }

    #[test]
    fn branch_and_summary_lines_align_continuation_with_four_spaces() {
        let palette = default_palette();
        let item = AgentFactItem::launch(launch_snapshot(vec![
            launch_child(2, "a very long child task title that must wrap"),
            launch_child(
                3,
                "another long child task title that also wraps beyond the width",
            ),
        ]));
        let lines = item.render_lines(24, palette);
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(plain_lines[0], "● Launched 2 agents");
        for line in plain_lines.iter().skip(1) {
            assert!(
                line.starts_with("    ") || line.starts_with("  ├ ") || line.starts_with("  └ "),
                "wrapped lines must keep the tree alignment: {line}"
            );
        }
        assert!(
            plain_lines.len() > 3,
            "long titles must wrap at narrow width"
        );
    }

    #[test]
    fn header_line_wraps_without_splitting_styled_chunks() {
        let palette = default_palette();
        let item = AgentFactItem::launch(launch_snapshot(vec![launch_child(
            2,
            "an extremely long single task title that cannot fit into a narrow terminal width",
        )]));
        let lines = item.render_lines(20, palette);
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert!(plain_lines.len() > 1, "long single title must wrap");
        assert!(plain_lines[0].starts_with("● Launched "));
        for line in &plain_lines {
            assert!(
                crate::display_width::display_width(line) <= 20,
                "wrapped line must respect the width: {line}"
            );
        }
    }

    #[test]
    fn render_cache_key_distinguishes_semantic_content() {
        let launch_a = AgentFactItem::launch(launch_snapshot(vec![launch_child(2, "task a")]));
        let launch_b = AgentFactItem::launch(launch_snapshot(vec![launch_child(2, "task b")]));
        let launch_repeat = AgentFactItem::launch(launch_snapshot(vec![launch_child(2, "task a")]));
        let outcome = AgentFactItem::outcome(outcome_snapshot(AgentOutcome::Completed));

        assert_eq!(
            launch_a.render_cache_key(),
            launch_repeat.render_cache_key()
        );
        assert_ne!(launch_a.render_cache_key(), launch_b.render_cache_key());
        assert_ne!(launch_a.render_cache_key(), outcome.render_cache_key());
        assert_ne!(
            outcome.render_cache_key(),
            AgentFactItem::outcome(outcome_snapshot(AgentOutcome::Failed)).render_cache_key(),
        );
    }

    #[test]
    fn item_rendering_is_deterministic_and_free_of_error_styling() {
        let palette = default_palette();
        let item = AgentFactItem::outcome(outcome_snapshot(AgentOutcome::Failed));

        // append-only 事实没有 mutator：重复渲染必须产出完全相同的结果。
        let first = item.render_lines(60, palette);
        let second = item.render_lines(60, palette);
        assert_eq!(first, second);

        // Agent 事实不得降级成 error-styled system message 的视觉语言。
        let plain = item.render_plain_text(60, palette);
        assert!(!plain.contains('■'));
        assert!(plain.contains("● Failed research task"));
    }
}
