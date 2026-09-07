use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use runtime_domain::agent::{AgentId, AgentOverviewRow};

use crate::{
    Model,
    agents_panel::{
        AGENTS_STATUS_COLUMN_WIDTH, AgentsPanelActivityFold, AgentsPanelState,
        agent_activity_summary_text, agent_status_dot_style, agent_status_dot_symbol,
        agent_status_label, agents_panel_list_page_size, format_agent_elapsed_ms,
        format_agent_token_usage, format_agent_tool_uses, pad_agents_status_column,
    },
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, primary_text_style, secondary_text_style,
        subtle_rule_line, surface_text_style, tertiary_text_style,
    },
};

/// 选中行行首指示前缀；未选中行使用等宽空白前缀，保证列几何一致。
/// 前缀宽度须与 `AGENTS_CURSOR_PREFIX_WIDTH` 相符（`▸` 为单宽符号）。
pub(super) const AGENTS_CURSOR_PREFIX: &str = "▸ ";
const AGENTS_CURSOR_PREFIX_BLANK: &str = "  ";
const AGENTS_CURSOR_PREFIX_WIDTH: usize = 2;
/// 状态点符号（`●` / `○`）占用的显示列宽。
const AGENTS_STATUS_DOT_WIDTH: usize = 1;
/// 行首固定前缀总宽：选中指示 + 状态点 + 点与状态文字的间隔。
const AGENTS_ROW_PREFIX_WIDTH: usize =
    AGENTS_CURSOR_PREFIX_WIDTH + AGENTS_STATUS_DOT_WIDTH + AGENTS_COLUMN_GAP;
/// 行右端保留的空白列；metrics 序列右对齐锚定在 `width - AGENTS_ROW_RIGHT_PADDING`。
pub(super) const AGENTS_ROW_RIGHT_PADDING: usize = 2;
const AGENTS_COLUMN_GAP: usize = 1;
const AGENTS_TITLE_MIN_WIDTH: usize = 16;
const AGENTS_TITLE_MAX_WIDTH: usize = 40;
const AGENTS_LATEST_MIN_WIDTH: usize = 8;
/// latest 列低于此宽度时整列隐藏（连 ellipsis 都放不下即无语义）。
const AGENTS_LATEST_MIN_VISIBLE_WIDTH: usize = 3;
/// metrics 内部分隔符：`·` 单宽，两侧各一空格。
const AGENTS_METRIC_SEPARATOR: &str = " · ";
const AGENTS_METRIC_SEPARATOR_WIDTH: usize = 3;

/// 活动折叠行前缀：缩进对齐主行 title 列起点，`↳ ` 之后接活动摘要。
/// 折叠区仅在宽度不低于 `AGENTS_ACTIVITY_FOLD_MIN_WIDTH` 时渲染，该区间内
/// 状态列恒为满宽，title 列起点因此是常量。
pub(super) fn agents_activity_fold_prefix() -> String {
    const INDENT_WIDTH: usize =
        AGENTS_ROW_PREFIX_WIDTH + AGENTS_STATUS_COLUMN_WIDTH + AGENTS_COLUMN_GAP;
    format!("{}↳ ", " ".repeat(INDENT_WIDTH))
}

impl Model {
    pub(crate) fn render_agents_panel_list(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.agents_panel.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = agents_panel_list_page_size(area.height);
        let width = usize::from(area.width);

        frame.render_widget(
            Paragraph::new(self.agents_panel_header_line(state, width)),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(width, self.palette)),
            chrome.header_rule,
        );

        let lines =
            self.agents_panel_body_lines(state, width, usize::from(chrome.body.height), page_size);
        frame.render_widget(AgentsPanelListWidget { lines: &lines }, chrome.body);

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                state.page_number(page_size),
                state.page_count(page_size),
                self.palette,
            )),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                agents_panel_list_footer_hint(state, area.width),
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }

    fn agents_panel_header_line(&self, state: &AgentsPanelState, width: usize) -> Line<'static> {
        let title = format!(
            "Agents ({} of {})",
            state.selected_position_label(),
            state.filtered_count()
        );
        let title_width = width.saturating_sub(2).max(1);
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(
                truncate_display_width_with_ellipsis(&title, title_width),
                primary_text_style(self.palette).bold(),
            ),
        ];
        if state.is_searching() || !state.search_query().is_empty() {
            spans.push(Span::styled(" · ", primary_text_style(self.palette).bold()));
            spans.push(Span::styled(
                "Search:",
                crate::theme::command_accent_text_style(self.palette).bold(),
            ));
            spans.push(Span::styled(
                format!(" {}", state.search_query()),
                primary_text_style(self.palette).bold(),
            ));
        }
        Line::from(spans)
    }

    fn agents_panel_body_lines(
        &self,
        state: &AgentsPanelState,
        width: usize,
        body_height: usize,
        page_size: usize,
    ) -> Vec<Line<'static>> {
        let width = width.max(1);
        let mut lines = Vec::new();

        if state.is_loading {
            lines.push(Line::styled(
                "  Loading agents...",
                tertiary_text_style(self.palette),
            ));
        } else if let Some(error) = state.error.as_deref() {
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(&format!("  {error}"), width),
                tertiary_text_style(self.palette),
            ));
        } else if !state.has_rows() {
            lines.push(Line::styled(
                "  No child agents",
                tertiary_text_style(self.palette),
            ));
        } else if !state.has_filtered_rows() {
            let empty_message = if state.search_query().is_empty() {
                "  No child agents"
            } else {
                "  No agents match search"
            };
            lines.push(Line::styled(
                truncate_display_width_with_ellipsis(empty_message, width),
                tertiary_text_style(self.palette),
            ));
        } else {
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let Some(row) = state.row(row_index) else {
                    continue;
                };
                let absolute_position = page_start + visible_position;
                let is_cursor = state.is_selected_visible_position(absolute_position);
                lines.push(agents_panel_row_line(
                    row,
                    width,
                    is_cursor,
                    absolute_position.is_multiple_of(2),
                    self.palette,
                ));
                if is_cursor {
                    lines.extend(agents_panel_activity_fold_lines(
                        state.selected_activity_fold(),
                        row.agent_id,
                        width,
                        self.palette,
                    ));
                }
            }
        }

        lines.truncate(body_height);
        lines
    }
}

/// 固定单行 row：选中指示 + 状态点/文字 + title 主导列 + latest 弹性列 +
/// 右对齐 metrics 紧凑序列。
/// 选中只改变行首 `▸` 前缀与 title bold，不做整行反色，
/// 各列保持自己的语义色与斑马纹背景。
pub(super) fn agents_panel_row_line(
    row: &AgentOverviewRow,
    width: usize,
    is_cursor: bool,
    is_even: bool,
    palette: TerminalPalette,
) -> Line<'static> {
    let layout = agents_panel_row_layout(row, width);
    let row_style = agents_panel_row_style(palette, is_even);
    let status_style = agent_status_dot_style(row.status, &palette);
    let title_style = if is_cursor {
        primary_text_style(palette).add_modifier(Modifier::BOLD)
    } else {
        primary_text_style(palette)
    };

    // metrics 右对齐锚点：先量好锚点前的内容宽度（含实际存在的列间隔），
    // 剩余空隙全部前置填充。
    let title_width = display_width(&layout.title);
    let latest_width = layout.latest.as_deref().map_or(0, display_width);
    // 与下方 span 构造保持一致：空列不占用间隔。
    let title_gap = usize::from(!layout.title.is_empty()) * AGENTS_COLUMN_GAP;
    let latest_gap = usize::from(layout.latest.is_some()) * AGENTS_COLUMN_GAP;
    let content_width = AGENTS_ROW_PREFIX_WIDTH
        + display_width(&layout.status)
        + title_gap
        + title_width
        + latest_gap
        + latest_width
        + layout.metrics_width;
    let metrics_filler = width
        .saturating_sub(AGENTS_ROW_RIGHT_PADDING)
        .saturating_sub(content_width);

    let cursor_prefix = if is_cursor {
        AGENTS_CURSOR_PREFIX
    } else {
        AGENTS_CURSOR_PREFIX_BLANK
    };
    let mut spans = vec![
        Span::raw(cursor_prefix),
        Span::styled(agent_status_dot_symbol(row.status, &palette), status_style),
        Span::raw(" ".repeat(AGENTS_COLUMN_GAP)),
        Span::styled(layout.status, status_style),
    ];
    if !layout.title.is_empty() {
        spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
        spans.push(Span::styled(layout.title, title_style));
    }
    if let Some(latest) = layout.latest {
        spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
        spans.push(Span::styled(latest, secondary_text_style(palette)));
    }
    if !layout.metrics.is_empty() {
        if metrics_filler > 0 {
            spans.push(Span::raw(" ".repeat(metrics_filler)));
        }
        for (index, metric) in layout.metrics.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(AGENTS_METRIC_SEPARATOR));
            }
            spans.push(Span::styled(metric, tertiary_text_style(palette)));
        }
    }

    Line::from(spans).style(row_style)
}

/// 选中行下方的活动折叠区行：最多 3 条最近活动 + `+N more`。
///
/// tertiary 色、不参与斑马纹（不携带主行背景）；缓存归属与行 agent 脱节、
/// 宽度低于阈值或无条目时返回空。折叠区不改变主行的列布局。
fn agents_panel_activity_fold_lines(
    fold: &AgentsPanelActivityFold,
    agent_id: AgentId,
    width: usize,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if fold.agent_id != Some(agent_id) || fold.visible_line_count(width) == 0 {
        return Vec::new();
    }
    let style = tertiary_text_style(palette);
    let prefix = agents_activity_fold_prefix();
    let entry_width = width.saturating_sub(display_width(&prefix)).max(1);
    let mut lines: Vec<Line<'static>> = fold
        .entries
        .iter()
        .map(|entry| {
            Line::styled(
                format!(
                    "{prefix}{}",
                    truncate_display_width_with_ellipsis(entry, entry_width)
                ),
                style,
            )
        })
        .collect();
    if fold.more_count > 0 {
        lines.push(Line::styled(
            format!("{prefix}+{} more", fold.more_count),
            style,
        ));
    }
    lines
}

struct AgentsPanelListWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for AgentsPanelListWidget<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::buffer::Buffer) {
        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        }
    }
}

/// 单行 row 的列布局结果：各列文本 + 右对齐 metrics 紧凑序列。
/// `metrics_width` 是序列总显示宽（含 ` · ` 分隔符），行组合时据此做右对齐锚定。
pub(super) struct AgentsPanelRowLayout {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) latest: Option<String>,
    /// 幸存 metric 标签，按 elapsed → tools → tokens 顺序排列。
    pub(super) metrics: Vec<String>,
    pub(super) metrics_width: usize,
}

/// 职责分档布局：固定前缀（选中指示 + 状态点 + 状态文字）→ title 主导弹性列
/// → latest 弹性列 → metrics 从行右端预留（` · ` 紧凑连接、右对齐）。
/// 收窄让位顺序 tokens → tools → elapsed；极窄回退仅保留前缀 + 状态 + 标题。
pub(super) fn agents_panel_row_layout(
    row: &AgentOverviewRow,
    width: usize,
) -> AgentsPanelRowLayout {
    let status_budget = width.saturating_sub(AGENTS_ROW_PREFIX_WIDTH);
    let status = pad_agents_status_column(agent_status_label(row.status), status_budget);
    let status_width = display_width(&status);
    // 固定前缀：行首指示 + 状态点 + 状态文字 + 与后续列的间隔。
    let fixed_width = AGENTS_ROW_PREFIX_WIDTH + status_width + AGENTS_COLUMN_GAP;
    let title_text = row.title.as_str();
    let latest_text = agent_activity_summary_text(&row.latest_activity);

    // 只收集 row 数据里存在的 metric；让位时从末尾（优先级最低）开始丢弃。
    let mut metrics: Vec<String> = Vec::new();
    if let Some(elapsed_ms) = row.elapsed_ms {
        metrics.push(format_agent_elapsed_ms(elapsed_ms));
    }
    if let Some(tool_uses) = row.tool_uses {
        metrics.push(format_agent_tool_uses(tool_uses));
    }
    if let Some(token_usage) = row.token_usage {
        metrics.push(format_agent_token_usage(token_usage));
    }

    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);
    // 全列放不下时按 tokens → tools → elapsed 逐列丢弃；
    // 判定基准是 title/latest 的保底宽 + metrics 序列（含分隔与前置间隔）。
    let metrics_width = loop {
        let sequence_width = agents_metrics_sequence_width(&metrics);
        let metrics_gap = usize::from(!metrics.is_empty()) * AGENTS_COLUMN_GAP;
        let mandatory = fixed_width
            + AGENTS_TITLE_MIN_WIDTH
            + AGENTS_COLUMN_GAP
            + AGENTS_LATEST_MIN_WIDTH
            + metrics_gap
            + sequence_width;
        if metrics.is_empty() || usable_width >= mandatory {
            break sequence_width;
        }
        metrics.pop();
    };
    let metrics_gap = usize::from(!metrics.is_empty()) * AGENTS_COLUMN_GAP;
    // metrics 从右端预留后，title/latest 分享的弹性预算。
    let rest = usable_width.saturating_sub(fixed_width + metrics_gap + metrics_width);

    let (title, latest) = if rest <= AGENTS_TITLE_MIN_WIDTH {
        // 极窄回退：仅前缀 + 状态 + 标题，title 安全截断。
        (truncate_display_width_with_ellipsis(title_text, rest), None)
    } else {
        let flexible = rest - AGENTS_COLUMN_GAP;
        // title 主导：优先吃满弹性预算（受 max 上限约束），latest 只保底 min。
        let title_width = display_width(title_text)
            .min(AGENTS_TITLE_MAX_WIDTH)
            .min(flexible.saturating_sub(AGENTS_LATEST_MIN_WIDTH))
            .max(AGENTS_TITLE_MIN_WIDTH);
        let latest_width = flexible.saturating_sub(title_width);
        (
            truncate_display_width_with_ellipsis(title_text, title_width),
            (latest_width >= AGENTS_LATEST_MIN_VISIBLE_WIDTH)
                .then(|| truncate_display_width_with_ellipsis(&latest_text, latest_width)),
        )
    };

    AgentsPanelRowLayout {
        status,
        title,
        latest,
        metrics,
        metrics_width,
    }
}

/// metrics 紧凑序列的总显示宽：标签宽之和 + 相邻 ` · ` 分隔符。
fn agents_metrics_sequence_width(metrics: &[String]) -> usize {
    let labels_width: usize = metrics.iter().map(|label| display_width(label)).sum();
    let separators_width = metrics
        .len()
        .saturating_sub(1)
        .saturating_mul(AGENTS_METRIC_SEPARATOR_WIDTH);
    labels_width + separators_width
}

/// 斑马纹偶数行使用 surface 背景，与 message history 列表一致。
fn agents_panel_row_style(palette: TerminalPalette, is_even: bool) -> Style {
    if is_even {
        surface_text_style(palette)
    } else {
        Style::new()
    }
}

fn agents_panel_list_footer_hint(state: &AgentsPanelState, width: u16) -> String {
    // stop 确认提示优先于常规 hint：全屏层下全局 status notice 不可见。
    if let Some(confirmed) = state.stop_confirmation
        && let Some(title) = state.row_title(confirmed)
    {
        return truncate_display_width_with_ellipsis(
            &format!("  Press x again to stop {title}"),
            usize::from(width),
        );
    }
    if width < 90 {
        "  Esc close · Space preview · Enter transcript · x stop · / search · j/k move".to_string()
    } else {
        "  Esc close · Space preview · Enter transcript · x stop · / search · ↑/↓/j/k move · ←/→/h/l page".to_string()
    }
}
