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
        AGENTS_ACTIVITY_FOLD_MIN_WIDTH, AGENTS_ELAPSED_COLUMN_WIDTH, AGENTS_STATUS_COLUMN_WIDTH,
        AGENTS_TOKENS_COLUMN_WIDTH, AGENTS_TOOLS_COLUMN_WIDTH, AgentsPanelActivityFold,
        AgentsPanelPageBodyLine, AgentsPanelState, AgentsPanelStopConfirmation,
        agent_activity_summary_text, agent_status_dot_style, agent_status_dot_symbol,
        agent_status_is_running, agent_status_is_settled, agent_status_label,
        agents_panel_list_page_size, format_agent_token_usage, format_agent_tool_uses,
        groups::{AgentsRowGroupKind, agents_panel_now_unix_ms},
    },
    display_width::display_width,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    search_highlight::{highlighted_substring_spans, search_match_style},
    status_line::{truncate_display_width, truncate_display_width_with_ellipsis},
    stream_activity::format_elapsed_compact,
    theme::{
        TerminalPalette, build_page_rule, command_accent_text_style, primary_text_style,
        secondary_text_style, subtle_rule_line, table_header_text_style, tertiary_text_style,
    },
};

/// 行首选中 marker：`█`（command_accent）+ 1 gap；未选中用等宽空白保持列几何。
const AGENTS_SELECTION_MARKER_WIDTH: usize = 2;
/// 状态点符号（`●` / `○`）占用的显示列宽；状态点归入 status 列，不占 marker 前缀。
const AGENTS_STATUS_DOT_WIDTH: usize = 1;
/// status 列内状态点前缀的总宽：dot 符号 + dot 与状态文字的间隔。
/// 状态文字在 status 列内偏移该宽度；列头 "Status" 与状态点共用 status 列起点。
const AGENTS_STATUS_COLUMN_PREFIX_WIDTH: usize = AGENTS_STATUS_DOT_WIDTH + AGENTS_COLUMN_GAP;
/// 行首固定前缀总宽：仅选中 marker；状态点与状态文字一起计入 status 列宽。
const AGENTS_ROW_PREFIX_WIDTH: usize = AGENTS_SELECTION_MARKER_WIDTH;
/// 行右端保留的空白列；metrics 列右对齐锚定在 `width - AGENTS_ROW_RIGHT_PADDING`。
pub(super) const AGENTS_ROW_RIGHT_PADDING: usize = 2;
const AGENTS_COLUMN_GAP: usize = 1;
/// `x` 二次确认的内联提示文案：占用选中行 latest 列槽位（Idle 行同样显示，
/// 提示优先于活动文本），command_accent 着色。footer 不再承担该提示。
/// stop 与 delete 的提示文案分开，确认态向用户声明本次动作语义；Sentence case
/// 对齐 exit confirmation 系列（"Press again to exit"）。
pub(super) const AGENTS_STOP_CONFIRM_HINT: &str = "· Press x again to stop";
pub(super) const AGENTS_DELETE_CONFIRM_HINT: &str = "· Press x again to delete";
/// metric 列之间的间隔：固定列宽下纵向对齐由列边界承载，不需要 `·` 分隔。
const AGENTS_METRIC_COLUMN_GAP: usize = 1;
/// metrics 列数（elapsed / tools / tokens）。
const AGENTS_METRIC_COLUMN_COUNT: usize = 3;
/// metrics 三列的固定列宽；下标与槽位一一对应（0/1/2 = elapsed/tools/tokens）。
const AGENTS_METRIC_COLUMN_WIDTHS: [usize; AGENTS_METRIC_COLUMN_COUNT] = [
    AGENTS_ELAPSED_COLUMN_WIDTH,
    AGENTS_TOOLS_COLUMN_WIDTH,
    AGENTS_TOKENS_COLUMN_WIDTH,
];
/// metrics 三列的列头标签；下标与 `AGENTS_METRIC_COLUMN_WIDTHS` 一一对应。
const AGENTS_METRIC_COLUMN_HEADER_LABELS: [&str; AGENTS_METRIC_COLUMN_COUNT] =
    ["Time", "Tools", "Tokens"];
const AGENTS_TITLE_MIN_WIDTH: usize = 16;
const AGENTS_TITLE_MAX_WIDTH: usize = 40;
const AGENTS_LATEST_MIN_WIDTH: usize = 8;
/// latest 列低于此宽度时整列隐藏（连 ellipsis 都放不下即无语义）。
const AGENTS_LATEST_MIN_VISIBLE_WIDTH: usize = 3;

/// 单列槽位：列内右对齐内容的行内起点与固定列宽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AgentsPanelColumnSlot {
    pub(super) start: usize,
    pub(super) width: usize,
}

/// `/agents` 行几何：列头行与数据行共用的单一列推导结果。
///
/// 位置均为行内显示列（从 0 起）。title/latest 是弹性列——title 起点固定，
/// latest 跟随 title 实际内容浮动（无固定起点可标），弹性分配在行布局内完成，
/// 几何只提供弹性总预算。metrics 三列固定列宽、右端锚定
/// `width - AGENTS_ROW_RIGHT_PADDING`，收窄时按 tokens → tools → elapsed
/// 顺序整列让位（让位列不出现槽位）。
#[derive(Debug, Clone, Copy)]
pub(super) struct AgentsPanelRowGeometry {
    /// status 列起点（行首 marker 之后）；数据行状态点与列头 "Status" 共用。
    pub(super) status_start: usize,
    /// status 列内状态文字的列宽（dot + 间隔之后）。
    pub(super) status_text_width: usize,
    /// title 列起点（status 列 + 间隔之后）。
    pub(super) title_start: usize,
    /// title + latest 的弹性总预算（含两列之间的间隔）。
    pub(super) elastic_width: usize,
    /// metrics 三列槽位（下标 0/1/2 = elapsed/tools/tokens）；让位列为 `None`。
    pub(super) metric_slots: [Option<AgentsPanelColumnSlot>; AGENTS_METRIC_COLUMN_COUNT],
}

/// 列头行与数据行共用的列几何推导：固定前缀（marker + status 列）→ metrics
/// 固定列宽序列（右端锚定、收窄让位）→ 剩余弹性预算。
pub(super) fn agents_panel_row_geometry(width: usize) -> AgentsPanelRowGeometry {
    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);
    let status_text_width = AGENTS_STATUS_COLUMN_WIDTH
        .min(width.saturating_sub(AGENTS_ROW_PREFIX_WIDTH + AGENTS_STATUS_COLUMN_PREFIX_WIDTH));
    let title_start = AGENTS_ROW_PREFIX_WIDTH
        + AGENTS_STATUS_COLUMN_PREFIX_WIDTH
        + status_text_width
        + AGENTS_COLUMN_GAP;

    // 收窄让位：预算不足时从 tokens 起整列丢弃；判定基准含 title/latest 保底宽。
    let mut visible_count = AGENTS_METRIC_COLUMN_COUNT;
    while visible_count > 0 {
        let mandatory = title_start
            + AGENTS_TITLE_MIN_WIDTH
            + AGENTS_COLUMN_GAP
            + AGENTS_LATEST_MIN_WIDTH
            + AGENTS_COLUMN_GAP
            + metric_sequence_width(&AGENTS_METRIC_COLUMN_WIDTHS[..visible_count]);
        if usable_width >= mandatory {
            break;
        }
        visible_count -= 1;
    }

    // 幸存列从右锚点逆序定位（tokens 最靠右），列间单空格。
    let mut metric_slots = [None; AGENTS_METRIC_COLUMN_COUNT];
    let mut anchor = usable_width;
    for index in (0..visible_count).rev() {
        let slot_width = AGENTS_METRIC_COLUMN_WIDTHS[index];
        anchor = anchor.saturating_sub(slot_width);
        metric_slots[index] = Some(AgentsPanelColumnSlot {
            start: anchor,
            width: slot_width,
        });
        anchor = anchor.saturating_sub(AGENTS_METRIC_COLUMN_GAP);
    }

    let metrics_width = metric_sequence_width(&AGENTS_METRIC_COLUMN_WIDTHS[..visible_count]);
    let metrics_gap = usize::from(visible_count > 0) * AGENTS_COLUMN_GAP;

    AgentsPanelRowGeometry {
        status_start: AGENTS_ROW_PREFIX_WIDTH,
        status_text_width,
        title_start,
        elastic_width: usable_width.saturating_sub(title_start + metrics_gap + metrics_width),
        metric_slots,
    }
}

/// metric 序列总宽：各列宽之和 + 列间单空格。
fn metric_sequence_width(column_widths: &[usize]) -> usize {
    column_widths.iter().sum::<usize>()
        + column_widths.len().saturating_sub(1) * AGENTS_METRIC_COLUMN_GAP
}

/// 活动折叠行前缀：缩进对齐主行 title 列起点，符号之后接活动摘要。
/// 非最后行用竖线 `│`（连续展开视觉），仅最后一行用 `↳`。
/// 折叠区仅在宽度不低于 `AGENTS_ACTIVITY_FOLD_MIN_WIDTH` 时渲染，该区间内
/// 状态列恒为满宽，title 列起点因此恒定；缩进经同一几何推导取得。
pub(super) fn agents_activity_fold_prefix(symbol: &str) -> String {
    let indent = agents_panel_row_geometry(AGENTS_ACTIVITY_FOLD_MIN_WIDTH).title_start;
    format!("{}{symbol} ", " ".repeat(indent))
}

impl Model {
    pub(crate) fn render_agents_panel_list(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if self.agents_panel.is_none() {
            return;
        }
        // 分组随墙钟迁移：渲染前先把行序归一到当前分组（selection 以 id 重锚），
        // 分页与组头推导才与本帧的分组一致。
        let now_ms = agents_panel_now_unix_ms();
        if let Some(panel) = self.agents_panel.as_mut() {
            panel.refresh_display_order(now_ms);
        }
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = agents_panel_list_page_size(area.height);
        let width = usize::from(area.width);

        let Some(state) = self.agents_panel.as_ref() else {
            return;
        };
        frame.render_widget(
            Paragraph::new(self.agents_panel_header_line(state, width)),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(width, self.palette)),
            chrome.header_rule,
        );

        let lines = self.agents_panel_body_lines(
            state,
            width,
            usize::from(chrome.body.height),
            page_size,
            now_ms,
        );
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
        now_ms: i64,
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
            // 之后按组渲染：组头行 → 组内数据行 → 下一组；组头不可选，
            // 计划与鼠标物理行换算共用 `page_body_line_plan`。
            for line in state.page_body_line_plan(page_size, now_ms) {
                match line {
                    AgentsPanelPageBodyLine::GroupHeader { kind, row_count } => {
                        lines.push(agents_panel_group_header_line(
                            kind,
                            row_count,
                            width,
                            self.palette,
                        ));
                    }
                    AgentsPanelPageBodyLine::Row { position } => {
                        let Some(row) = state.filtered_row_at(position) else {
                            continue;
                        };
                        let is_cursor = state.is_selected_visible_position(position);
                        let confirm_hint = is_cursor
                            .then(|| match state.stop_confirmation {
                                Some(confirmation) if confirmation.agent_id() == row.agent_id => {
                                    Some(agents_panel_confirm_hint_text(confirmation))
                                }
                                _ => None,
                            })
                            .flatten();
                        lines.push(agents_panel_row_line(
                            row,
                            width,
                            is_cursor,
                            confirm_hint,
                            state.search_query(),
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
            }
        }

        lines.truncate(body_height);
        lines
    }
}

/// 组头行：组名 + 过滤视图内组行数（如 `Running (2)`）。
///
/// `table_header` 样式与列头一致，起点对齐列头的 status 列；整行文本不参与
/// 列几何（无列对齐诉求），按行宽安全截断。组头行不可选，也不计入
/// `N of M` 的可选行计数。
pub(super) fn agents_panel_group_header_line(
    kind: AgentsRowGroupKind,
    row_count: usize,
    width: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = width.max(1);
    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);
    let text = format!(
        "{}{} ({})",
        " ".repeat(AGENTS_ROW_PREFIX_WIDTH),
        kind.header_label(),
        row_count
    );
    // style 落在 span 上（与列头行同构，便于 span 断言）。
    Line::from(vec![Span::styled(
        truncate_display_width_with_ellipsis(&text, usable_width),
        table_header_text_style(palette),
    )])
}

/// 确认态动作对应的内联提示文案。
fn agents_panel_confirm_hint_text(confirmation: AgentsPanelStopConfirmation) -> &'static str {
    match confirmation {
        AgentsPanelStopConfirmation::Stop(_) => AGENTS_STOP_CONFIRM_HINT,
        AgentsPanelStopConfirmation::Delete(_) => AGENTS_DELETE_CONFIRM_HINT,
    }
}

/// 固定单行 row：选中 marker + status 列（状态点 + 状态文字）+ title 主导列 +
/// latest 弹性列 + 右对齐固定列宽 metrics。
/// 选中只改变行首 `█` marker 与 title bold；行不携带背景（无斑马纹），
/// 各列保持自己的语义色。title 的搜索命中以 surface 背景强调（session picker
/// 同款高亮）。确认提示激活时 latest 列被内联提示接管（command_accent 着色），
/// 列几何不变。
pub(super) fn agents_panel_row_line(
    row: &AgentOverviewRow,
    width: usize,
    is_cursor: bool,
    confirm_hint: Option<&'static str>,
    search_query: &str,
    palette: TerminalPalette,
) -> Line<'static> {
    let layout = agents_panel_row_layout(row, width, confirm_hint);
    let geometry = agents_panel_row_geometry(width);
    let status_style = agent_status_dot_style(row.status, &palette);
    let title_style = if is_cursor {
        primary_text_style(palette).add_modifier(Modifier::BOLD)
    } else {
        primary_text_style(palette)
    };

    // metrics 右对齐锚点：首个幸存列的槽位起点减去锚点前内容宽，剩余空隙前置填充；
    // metrics 段自身列宽固定，行尾恒终止在锚点。锚点前内容按 span 实宽求和
    // （title 为空时不渲染 title 前的间隔列，与下方构造保持一致）。
    let title_width = display_width(&layout.title);
    let latest_width = layout.latest.as_deref().map_or(0, display_width);
    let title_gap = usize::from(!layout.title.is_empty()) * AGENTS_COLUMN_GAP;
    let latest_gap = usize::from(layout.latest.is_some()) * AGENTS_COLUMN_GAP;
    let content_width = AGENTS_ROW_PREFIX_WIDTH
        + AGENTS_STATUS_COLUMN_PREFIX_WIDTH
        + display_width(&layout.status)
        + title_gap
        + title_width
        + latest_gap
        + latest_width;
    let metrics_filler = geometry
        .metric_slots
        .iter()
        .flatten()
        .next()
        .map_or(0, |slot| slot.start.saturating_sub(content_width));

    let mut spans = vec![
        agents_panel_selection_marker_span(is_cursor, palette),
        Span::styled(agent_status_dot_symbol(row.status, &palette), status_style),
        Span::raw(" ".repeat(AGENTS_COLUMN_GAP)),
        Span::styled(layout.status, status_style),
    ];
    if !layout.title.is_empty() {
        spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
        spans.extend(highlighted_substring_spans(
            &layout.title,
            search_query,
            title_style,
            search_match_style(title_style, palette.surface),
        ));
    }
    if let Some(latest) = layout.latest {
        spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
        let latest_style = if confirm_hint.is_some() {
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

/// 列头行：与数据行共用 `agents_panel_row_geometry` 的单一列几何——
/// "Status" 与状态点同起点、"Title" 标注 title 列起点、metrics 列名右对齐进
/// 各自槽位（与行数值同锚点）。latest 列不标列头（内容跟随 title 浮动，无固定
/// 起点可标）。整行 ellipsis 安全截断。
pub(super) fn agents_panel_column_header_line(
    width: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = width.max(1);
    let geometry = agents_panel_row_geometry(width);
    let usable_width = width.saturating_sub(AGENTS_ROW_RIGHT_PADDING);

    let mut text = String::new();
    text.push_str(&" ".repeat(geometry.status_start));
    text.push_str("Status");
    let title_gap = geometry
        .title_start
        .saturating_sub(geometry.status_start + display_width("Status"));
    text.push_str(&" ".repeat(title_gap));
    text.push_str("Title");

    let mut cursor = geometry.title_start + display_width("Title");
    for (slot, label) in geometry
        .metric_slots
        .iter()
        .zip(AGENTS_METRIC_COLUMN_HEADER_LABELS)
    {
        let Some(slot) = slot else {
            continue;
        };
        text.push_str(&" ".repeat(slot.start.saturating_sub(cursor)));
        text.push_str(&align_metric_to_column(label, slot.width));
        cursor = slot.start + slot.width;
    }

    // style 落在 span 上（`Line::styled` 挂 line 层，不进 span 断言）。
    Line::from(vec![Span::styled(
        truncate_display_width_with_ellipsis(&text, usable_width),
        table_header_text_style(palette),
    )])
}

/// 选中行下方的活动折叠区行：最多 3 条最近活动 + `+N more`。
///
/// tertiary 色、不携带背景；仅在选中行上按 Tab 展开后渲染。缓存归属与行
/// agent 脱节、宽度低于阈值或无条目时返回空（归属/可见性判定与鼠标物理行
/// 换算共用 `AgentsPanelActivityFold::visible_line_count`）。折叠区不改变
/// 主行的列布局。
fn agents_panel_activity_fold_lines(
    fold: &AgentsPanelActivityFold,
    agent_id: AgentId,
    width: usize,
    expanded: bool,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    if fold.visible_line_count(agent_id, width, expanded) == 0 {
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
        // `+N more` 计数段用 command_accent 强调，折叠前缀保持 tertiary
        //（对齐 prompt overlay footer 的 more 段着色形态）。
        lines.push(Line::from(vec![
            Span::styled(agents_activity_fold_prefix("↳"), style),
            Span::styled(
                format!("+{} more", fold.more_count),
                command_accent_text_style(palette),
            ),
        ]));
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
pub(super) struct AgentsPanelRowLayout {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) latest: Option<String>,
    /// 幸存 metric 列（已按列宽右对齐填充），按 elapsed → tools → tokens 顺序排列。
    /// 值缺失的列为等宽空格占位——列集合只由宽度决定，行间纵向对齐。
    pub(super) metrics: Vec<String>,
}

/// 职责分档布局：固定前缀（选中 marker + status 列：状态点 + 状态文字）→
/// title 主导弹性列 → latest 弹性列 → metrics 固定列宽右对齐锚定行右端。
/// 收窄让位顺序 tokens → tools → elapsed；极窄回退仅保留前缀 + 状态 + 标题。
/// 确认提示激活时 latest 列槽位固定给内联提示（Idle 行同样显示，提示
/// 优先于活动文本），截断规则与普通 latest 一致，列几何不受影响。
pub(super) fn agents_panel_row_layout(
    row: &AgentOverviewRow,
    width: usize,
    confirm_hint: Option<&str>,
) -> AgentsPanelRowLayout {
    let geometry = agents_panel_row_geometry(width);
    let status =
        pad_agents_status_column(agent_status_label(row.status), geometry.status_text_width);
    let title_text = row.title.as_str();
    // Idle 不携带有效信息：latest 列整列隐藏（行保留其余列），不渲染占位文本。
    let latest_text = confirm_hint
        .map(str::to_string)
        .or_else(|| match &row.latest_activity {
            runtime_domain::agent::AgentActivitySummary::Idle => None,
            activity => Some(agent_activity_summary_text(activity)),
        });

    // metrics 槽位恒定：值缺失的列渲染等宽空格占位（新 agent 0 工具 0 token 是
    // 常态），列集合不随行数据变化。
    let metric_labels = [
        row.elapsed_ms.map(|ms| format_elapsed_compact(ms / 1_000)),
        row.tool_uses.map(format_agent_tool_uses),
        row.token_usage.map(format_agent_token_usage),
    ];
    let metrics = geometry
        .metric_slots
        .iter()
        .zip(metric_labels)
        .filter_map(|(slot, label)| slot.map(|slot| (slot, label)))
        .map(|(slot, label)| match label {
            Some(label) => align_metric_to_column(&label, slot.width),
            None => " ".repeat(slot.width),
        })
        .collect::<Vec<_>>();

    let rest = geometry.elastic_width;
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
    }
}

/// status 列固定宽度填充：所有行的 status 标签占同一列宽，后续列纵向对齐。
fn pad_agents_status_column(label: &str, width_budget: usize) -> String {
    let column_width = AGENTS_STATUS_COLUMN_WIDTH.min(width_budget);
    let label = truncate_display_width(label, column_width);
    let padding = column_width.saturating_sub(display_width(&label));
    format!("{label}{}", " ".repeat(padding))
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
    let mut parts = vec!["Esc close", "Space/Enter transcript"];
    // x 的动作语义跟随选中行状态；不可操作选区不渲染该段，footer 不预告无效动作。
    if let Some(x_action) = agents_panel_list_x_action_label(state) {
        parts.push(x_action);
    }
    parts.push("Tab fold");
    parts.push("/ search");
    if width < 90 {
        parts.push("j/k move");
    } else {
        parts.push("↑/↓/j/k move");
        parts.push("←/→/h/l page");
    }
    format!("  {}", parts.join(" · "))
}

/// footer 的 `x` 动作标签：running 行是 stop、settled 投影行是 delete——分类与
/// `handle_agents_panel_stop_key` 同源（CleanupBlocked 行不可操作，不渲染）。
fn agents_panel_list_x_action_label(state: &AgentsPanelState) -> Option<&'static str> {
    let row = state.selected_row()?;
    if agent_status_is_running(row.status) {
        Some("x stop")
    } else if agent_status_is_settled(row.status) {
        Some("x delete")
    } else {
        None
    }
}
