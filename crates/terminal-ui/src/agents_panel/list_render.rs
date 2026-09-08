use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use runtime_domain::agent::{AgentId, AgentOverviewRow};

use crate::{
    Model,
    agents_panel::{
        AGENTS_ELAPSED_COLUMN_WIDTH, AGENTS_STATUS_COLUMN_WIDTH, AGENTS_TOKENS_COLUMN_WIDTH,
        AGENTS_TOOLS_COLUMN_WIDTH, AgentsPanelActivityFold, AgentsPanelState,
        agent_activity_summary_text, agent_status_dot_style, agent_status_dot_symbol,
        agent_status_label, agents_panel_list_page_size, format_agent_elapsed_ms,
        format_agent_token_usage, format_agent_tool_uses, pad_agents_status_column,
    },
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    theme::{
        TerminalPalette, build_page_rule, command_accent_text_style, primary_text_style,
        secondary_text_style, subtle_rule_line, table_header_text_style, tertiary_text_style,
    },
};

/// 行首选中 marker：`█`（command_accent）+ 1 gap；未选中用等宽空白保持列几何。
const AGENTS_SELECTION_MARKER_WIDTH: usize = 2;
/// 状态点符号（`●` / `○`）占用的显示列宽。
const AGENTS_STATUS_DOT_WIDTH: usize = 1;
/// 行首固定前缀总宽：选中 marker + 状态点 + 点与状态文字的间隔。
const AGENTS_ROW_PREFIX_WIDTH: usize =
    AGENTS_SELECTION_MARKER_WIDTH + AGENTS_STATUS_DOT_WIDTH + AGENTS_COLUMN_GAP;
/// 行右端保留的空白列；metrics 列右对齐锚定在 `width - AGENTS_ROW_RIGHT_PADDING`。
pub(super) const AGENTS_ROW_RIGHT_PADDING: usize = 2;
const AGENTS_COLUMN_GAP: usize = 1;
/// stop 二次确认的内联提示文案：占用选中行 latest 列槽位（Idle 行同样显示，
/// 提示优先于活动文本），command_accent 着色。footer 不再承担该提示。
pub(super) const AGENTS_STOP_CONFIRM_HINT: &str = "· press x again to stop";
/// metric 列之间的间隔：固定列宽下纵向对齐由列边界承载，不需要 `·` 分隔。
const AGENTS_METRIC_COLUMN_GAP: usize = 1;
const AGENTS_TITLE_MIN_WIDTH: usize = 16;
const AGENTS_TITLE_MAX_WIDTH: usize = 40;
const AGENTS_LATEST_MIN_WIDTH: usize = 8;
/// latest 列低于此宽度时整列隐藏（连 ellipsis 都放不下即无语义）。
const AGENTS_LATEST_MIN_VISIBLE_WIDTH: usize = 3;

/// 活动折叠行前缀：缩进对齐主行 title 列起点，符号之后接活动摘要。
/// 非最后行用竖线 `│`（连续展开视觉），仅最后一行用 `↳`。
/// 折叠区仅在宽度不低于 `AGENTS_ACTIVITY_FOLD_MIN_WIDTH` 时渲染，该区间内
/// 状态列恒为满宽，title 列起点因此是常量。
pub(super) fn agents_activity_fold_prefix(symbol: &str) -> String {
    const INDENT_WIDTH: usize =
        AGENTS_ROW_PREFIX_WIDTH + AGENTS_STATUS_COLUMN_WIDTH + AGENTS_COLUMN_GAP;
    format!("{}{symbol} ", " ".repeat(INDENT_WIDTH))
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
            // 列头行占据 body 首行（对齐 prompt overlay / branch picker 的表头惯例），
            // 与 header_rule 一起把标题区与数据区隔开。
            lines.push(agents_panel_column_header_line(width, self.palette));
            let page_start = state.page_start(page_size);
            for (visible_position, row_index) in state.page_indices(page_size).enumerate() {
                let Some(row) = state.row(row_index) else {
                    continue;
                };
                let absolute_position = page_start + visible_position;
                let is_cursor = state.is_selected_visible_position(absolute_position);
                let stop_hint = is_cursor && state.stop_confirmation == Some(row.agent_id);
                lines.push(agents_panel_row_line(
                    row,
                    width,
                    is_cursor,
                    stop_hint,
                    self.palette,
                ));
                if is_cursor {
                    lines.extend(agents_panel_activity_fold_lines(
                        state.selected_activity_fold(),
                        row.agent_id,
                        width,
                        state.activity_fold_expanded,
                        self.palette,
                    ));
                }
            }
        }

        lines.truncate(body_height);
        lines
    }
}

/// 固定单行 row：选中 marker + 状态点/文字 + title 主导列 + latest 弹性列 +
/// 右对齐固定列宽 metrics。
/// 选中只改变行首 `█` marker 与 title bold；行不携带背景（无斑马纹），
/// 各列保持自己的语义色。stop 确认激活时 latest 列被内联提示接管
/// （command_accent 着色），列几何不变。
pub(super) fn agents_panel_row_line(
    row: &AgentOverviewRow,
    width: usize,
    is_cursor: bool,
    stop_hint: bool,
    palette: TerminalPalette,
) -> Line<'static> {
    let layout = agents_panel_row_layout(row, width, stop_hint);
    let status_style = agent_status_dot_style(row.status, &palette);
    let title_style = if is_cursor {
        primary_text_style(palette).add_modifier(Modifier::BOLD)
    } else {
        primary_text_style(palette)
    };

    // metrics 右对齐锚点：先量好锚点前的内容宽度，剩余空隙全部前置填充；
    // metrics 段自身列宽固定，行尾恒终止在锚点。
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

    let mut spans = vec![
        agents_panel_selection_marker_span(is_cursor, palette),
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
        let latest_style = if stop_hint {
            command_accent_text_style(palette)
        } else {
            secondary_text_style(palette)
        };
        spans.push(Span::styled(latest, latest_style));
    }
    if !layout.metrics.is_empty() {
        if metrics_filler > 0 {
            spans.push(Span::raw(" ".repeat(metrics_filler)));
        }
        for (index, metric) in layout.metrics.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" ".repeat(AGENTS_METRIC_COLUMN_GAP)));
            }
            spans.push(Span::styled(metric, tertiary_text_style(palette)));
        }
    }

    Line::from(spans)
}

/// 行首选中 marker span：session picker 同款 `█`（command_accent 色）。
/// 未选中 marker 为等宽空白，列几何不随选中变化。
fn agents_panel_selection_marker_span(is_cursor: bool, palette: TerminalPalette) -> Span<'static> {
    if is_cursor {
        Span::styled("█ ", command_accent_text_style(palette))
    } else {
        Span::raw("  ")
    }
}

/// 列头行：列名对齐行布局的列几何——状态列与行同宽左对齐、metrics 三列
/// 固定列宽右对齐锚定行右端；title/latest 为弹性列，列名只标列起点。
pub(super) fn agents_panel_column_header_line(
    width: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = width.max(1);
    let status = pad_agents_status_column("Status", width.saturating_sub(AGENTS_ROW_PREFIX_WIDTH));
    let fixed_width = AGENTS_ROW_PREFIX_WIDTH + display_width(&status) + AGENTS_COLUMN_GAP;
    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);
    let columns = agents_panel_metric_columns(
        Some(align_metric_to_column("Time", AGENTS_ELAPSED_COLUMN_WIDTH)),
        Some(align_metric_to_column("Tools", AGENTS_TOOLS_COLUMN_WIDTH)),
        Some(align_metric_to_column("Tokens", AGENTS_TOKENS_COLUMN_WIDTH)),
        usable_width,
        fixed_width,
    );

    let style = table_header_text_style(palette);
    let status_width = display_width(&status);
    let mut spans = vec![
        Span::raw(" ".repeat(AGENTS_ROW_PREFIX_WIDTH)),
        Span::styled(status, style),
        Span::raw(" ".repeat(AGENTS_COLUMN_GAP)),
        Span::styled("Title".to_string(), style),
        Span::raw(" ".repeat(AGENTS_COLUMN_GAP)),
        Span::styled("Latest".to_string(), style),
    ];
    if !columns.is_empty() {
        let leading_width = AGENTS_ROW_PREFIX_WIDTH
            + status_width
            + 2 * AGENTS_COLUMN_GAP
            + display_width("Title")
            + display_width("Latest");
        let columns_width = agents_metric_sequence_width(&columns);
        let filler = usable_width.saturating_sub(leading_width + columns_width);
        spans.push(Span::raw(" ".repeat(filler)));
        for (index, column) in columns.into_iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" ".repeat(AGENTS_METRIC_COLUMN_GAP)));
            }
            spans.push(Span::styled(column, style));
        }
    }
    Line::from(spans)
}

/// 选中行下方的活动折叠区行：最多 3 条最近活动 + `+N more`。
///
/// tertiary 色、不携带背景；仅在选中行上按 Tab 展开后渲染。缓存归属与行
/// agent 脱节、宽度低于阈值或无条目时返回空。折叠区不改变主行的列布局。
fn agents_panel_activity_fold_lines(
    fold: &AgentsPanelActivityFold,
    agent_id: AgentId,
    width: usize,
    expanded: bool,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if fold.agent_id != Some(agent_id) || fold.visible_line_count(width, expanded) == 0 {
        return Vec::new();
    }
    let style = tertiary_text_style(palette);
    let entry_width = width
        .saturating_sub(display_width(&agents_activity_fold_prefix("│")))
        .max(1);
    // 仅最后一行用 `↳`：有 more 行时 last entry 保持竖线、more 行收尾；
    // 无 more 行时 last entry 自身收尾。
    let has_more = fold.more_count > 0;
    let mut lines: Vec<Line<'static>> = fold
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let symbol = if !has_more && index + 1 == fold.entries.len() {
                "↳"
            } else {
                "│"
            };
            Line::styled(
                format!(
                    "{}{}",
                    agents_activity_fold_prefix(symbol),
                    truncate_display_width_with_ellipsis(entry, entry_width)
                ),
                style,
            )
        })
        .collect();
    if has_more {
        lines.push(Line::styled(
            format!(
                "{}+{} more",
                agents_activity_fold_prefix("↳"),
                fold.more_count
            ),
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
            buf.set_line(area.x, y, line, area.width);
        }
    }
}

/// 单行 row 的列布局结果：各列文本 + 右对齐固定列宽 metrics。
/// `metrics_width` 是列段总显示宽（含列间隔），行组合时据此做右对齐锚定。
pub(super) struct AgentsPanelRowLayout {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) latest: Option<String>,
    /// 幸存 metric 列（已按列宽右对齐填充），按 elapsed → tools → tokens 顺序排列。
    pub(super) metrics: Vec<String>,
    pub(super) metrics_width: usize,
}

/// 职责分档布局：固定前缀（选中 marker + 状态点 + 状态文字）→ title 主导弹性列
/// → latest 弹性列 → metrics 固定列宽右对齐锚定行右端。
/// 收窄让位顺序 tokens → tools → elapsed；极窄回退仅保留前缀 + 状态 + 标题。
/// `stop_hint` 激活时 latest 列槽位固定给内联提示（Idle 行同样显示，提示
/// 优先于活动文本），截断规则与普通 latest 一致，列几何不受影响。
pub(super) fn agents_panel_row_layout(
    row: &AgentOverviewRow,
    width: usize,
    stop_hint: bool,
) -> AgentsPanelRowLayout {
    let status_budget = width.saturating_sub(AGENTS_ROW_PREFIX_WIDTH);
    let status = pad_agents_status_column(agent_status_label(row.status), status_budget);
    let status_width = display_width(&status);
    // 固定前缀：行首 marker + 状态点 + 状态文字 + 与后续列的间隔。
    let fixed_width = AGENTS_ROW_PREFIX_WIDTH + status_width + AGENTS_COLUMN_GAP;
    let title_text = row.title.as_str();
    // Idle 不携带有效信息：latest 列整列隐藏（行保留其余列），不渲染占位文本。
    let latest_text = if stop_hint {
        Some(AGENTS_STOP_CONFIRM_HINT.to_string())
    } else {
        match &row.latest_activity {
            runtime_domain::agent::AgentActivitySummary::Idle => None,
            activity => Some(agent_activity_summary_text(activity)),
        }
    };

    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);
    let metrics = agents_panel_metric_columns(
        row.elapsed_ms.map(|ms| {
            align_metric_to_column(&format_agent_elapsed_ms(ms), AGENTS_ELAPSED_COLUMN_WIDTH)
        }),
        row.tool_uses.map(|uses| {
            align_metric_to_column(&format_agent_tool_uses(uses), AGENTS_TOOLS_COLUMN_WIDTH)
        }),
        row.token_usage.map(|usage| {
            align_metric_to_column(&format_agent_token_usage(usage), AGENTS_TOKENS_COLUMN_WIDTH)
        }),
        usable_width,
        fixed_width,
    );
    let metrics_width = agents_metric_sequence_width(&metrics);
    let metrics_gap = usize::from(!metrics.is_empty()) * AGENTS_COLUMN_GAP;
    // metrics 从右端锚定后，title/latest 分享的弹性预算。
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
            latest_text
                .filter(|text| {
                    display_width(text) > 0 && latest_width >= AGENTS_LATEST_MIN_VISIBLE_WIDTH
                })
                .map(|text| truncate_display_width_with_ellipsis(&text, latest_width)),
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

/// metric 列收集与收窄让位：每列固定列宽、右对齐、列间单空格。
/// 预算不足时按 tokens → tools → elapsed 从末尾整列丢弃；
/// 判定基准与行布局共用 title/latest 保底宽公式。
fn agents_panel_metric_columns(
    elapsed: Option<String>,
    tools: Option<String>,
    tokens: Option<String>,
    usable_width: usize,
    fixed_width: usize,
) -> Vec<String> {
    let mut columns: Vec<String> = [elapsed, tools, tokens].into_iter().flatten().collect();
    loop {
        let sequence_width = agents_metric_sequence_width(&columns);
        let metrics_gap = usize::from(!columns.is_empty()) * AGENTS_COLUMN_GAP;
        let mandatory = fixed_width
            + AGENTS_TITLE_MIN_WIDTH
            + AGENTS_COLUMN_GAP
            + AGENTS_LATEST_MIN_WIDTH
            + metrics_gap
            + sequence_width;
        if columns.is_empty() || usable_width >= mandatory {
            return columns;
        }
        columns.pop();
    }
}

/// metric 列段总显示宽：各列宽之和 + 列间单空格。
fn agents_metric_sequence_width(columns: &[String]) -> usize {
    columns
        .iter()
        .map(|column| display_width(column))
        .sum::<usize>()
        + columns.len().saturating_sub(1) * AGENTS_METRIC_COLUMN_GAP
}

/// metric 列内右对齐：不足列宽时前置填充；超宽标签原样保留
/// （极端数值不截断内容，该行列对齐暂时退化）。
fn align_metric_to_column(label: &str, column_width: usize) -> String {
    let label_width = display_width(label);
    if label_width >= column_width {
        label.to_string()
    } else {
        format!("{}{label}", " ".repeat(column_width - label_width))
    }
}

fn agents_panel_list_footer_hint(state: &AgentsPanelState, width: u16) -> String {
    // loading 期按下 x 后的可见回执：stop 暂不可用但按键不被静默吞掉。
    if state.stop_unavailable_notice {
        return truncate_display_width_with_ellipsis(
            "  Agents state is loading — x stop is unavailable yet",
            usize::from(width),
        );
    }
    if width < 90 {
        "  Esc close · Space/Enter transcript · x stop · Tab fold · / search · j/k move".to_string()
    } else {
        "  Esc close · Space/Enter transcript · x stop · Tab fold · / search · ↑/↓/j/k move · ←/→/h/l page".to_string()
    }
}
