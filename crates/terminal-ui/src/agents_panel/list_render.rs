use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};

use runtime_domain::agent::AgentOverviewRow;

use crate::{
    Model,
    agents_panel::{
        AGENTS_ELAPSED_COLUMN_WIDTH, AgentsPanelState, agent_activity_summary_text,
        agent_status_label, format_agent_elapsed_ms, format_agent_token_usage,
        format_agent_tool_uses, pad_agents_status_column,
    },
    display_width::display_width,
    fullscreen_list_chrome::{fullscreen_list_chrome_rects, fullscreen_list_page_size_for_height},
    relative_age::left_pad_display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, build_page_rule, primary_text_style, secondary_text_style,
        subtle_rule_line, surface_text_style, tertiary_text_style,
    },
};

pub(super) const AGENTS_ROW_LEFT_PADDING: &str = "  ";
const AGENTS_ROW_RIGHT_PADDING: usize = 2;
const AGENTS_COLUMN_GAP: usize = 1;
const AGENTS_TITLE_MIN_WIDTH: usize = 12;
const AGENTS_TITLE_MAX_WIDTH: usize = 32;
const AGENTS_LATEST_MIN_WIDTH: usize = 8;
/// latest 列低于此宽度时整列隐藏（连 ellipsis 都放不下即无语义）。
const AGENTS_LATEST_MIN_VISIBLE_WIDTH: usize = 3;

impl Model {
    pub(crate) fn render_agents_panel_list(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.agents_panel.as_ref() else {
            return;
        };
        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };
        let page_size = fullscreen_list_page_size_for_height(area.height);
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
                lines.push(self.agents_panel_row_line(
                    row,
                    width,
                    state.is_selected_visible_position(absolute_position),
                    absolute_position.is_multiple_of(2),
                ));
            }
        }

        lines.truncate(body_height);
        lines
    }

    /// 固定单行 row：status/title 恒显，latest 弹性，elapsed/tools/tokens 按剩余宽度让位。
    fn agents_panel_row_line(
        &self,
        row: &AgentOverviewRow,
        width: usize,
        is_cursor: bool,
        is_even: bool,
    ) -> Line<'static> {
        let layout = agents_panel_row_layout(row, width);
        let palette = self.palette;
        let row_style = agents_panel_row_style(palette, is_even);
        let content_style = if is_cursor {
            primary_text_style(palette).bold()
        } else {
            primary_text_style(palette)
        };
        // cursor 行整行反色：状态语义由文本承载，反色只承担焦点指示。
        let cursor_style = content_style
            .bg(Color::Reset)
            .add_modifier(Modifier::REVERSED);
        let status_style = if is_cursor {
            cursor_style
        } else {
            secondary_text_style(palette)
        };
        let body_style = if is_cursor {
            cursor_style
        } else {
            content_style
        };
        let metric_style = if is_cursor {
            cursor_style
        } else {
            tertiary_text_style(palette)
        };

        let mut spans = vec![
            Span::raw(AGENTS_ROW_LEFT_PADDING),
            Span::styled(layout.status, status_style),
        ];
        if !layout.title.is_empty() {
            spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
            spans.push(Span::styled(layout.title, body_style));
        }
        if let Some(latest) = layout.latest {
            spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
            spans.push(Span::styled(latest, body_style));
        }
        for metric in [layout.elapsed, layout.tools, layout.tokens]
            .into_iter()
            .flatten()
        {
            spans.push(Span::raw(" ".repeat(AGENTS_COLUMN_GAP)));
            spans.push(Span::styled(metric, metric_style));
        }

        Line::from(spans).style(row_style)
    }
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

/// 单行 row 的列布局结果。
struct AgentsPanelRowLayout {
    status: String,
    title: String,
    latest: Option<String>,
    elapsed: Option<String>,
    tools: Option<String>,
    tokens: Option<String>,
}

/// metric 列的固定让位顺序（末尾优先级最低）。
#[derive(Debug, Clone, Copy)]
enum AgentsMetricKind {
    Elapsed,
    Tools,
    Tokens,
}

/// 剩余宽度累加法：固定列（status）→ title（保底宽 + 截断）→ latest（弹性）→
/// elapsed → tools → tokens；放不下即止，行高恒 1。
fn agents_panel_row_layout(row: &AgentOverviewRow, width: usize) -> AgentsPanelRowLayout {
    let left_padding = AGENTS_ROW_LEFT_PADDING.len();
    let status_budget = width.saturating_sub(left_padding);
    let status = pad_agents_status_column(agent_status_label(row.status), status_budget);
    let status_width = display_width(&status);
    let title_text = row.title.as_str();
    let latest_text = agent_activity_summary_text(&row.latest_activity);

    // 只收集 row 数据里存在的 metric；让位时从末尾（优先级最低）开始丢弃。
    let mut metrics: Vec<(AgentsMetricKind, String, usize)> = Vec::new();
    if let Some(elapsed_ms) = row.elapsed_ms {
        metrics.push((
            AgentsMetricKind::Elapsed,
            left_pad_display_width(
                &format_agent_elapsed_ms(elapsed_ms),
                AGENTS_ELAPSED_COLUMN_WIDTH,
            ),
            AGENTS_ELAPSED_COLUMN_WIDTH,
        ));
    }
    if let Some(tool_uses) = row.tool_uses {
        let label = format_agent_tool_uses(tool_uses);
        let label_width = display_width(&label);
        metrics.push((AgentsMetricKind::Tools, label, label_width));
    }
    if let Some(token_usage) = row.token_usage {
        let label = format_agent_token_usage(token_usage);
        let label_width = display_width(&label);
        metrics.push((AgentsMetricKind::Tokens, label, label_width));
    }

    let body_budget = width
        .saturating_sub(left_padding + status_width + AGENTS_COLUMN_GAP + AGENTS_ROW_RIGHT_PADDING);

    // 先按全列判断，放不下则 tokens → tools → elapsed 逐列让位。
    loop {
        let metrics_width: usize = metrics
            .iter()
            .map(|(_, _, label_width)| AGENTS_COLUMN_GAP + label_width)
            .sum();
        let mandatory =
            AGENTS_TITLE_MIN_WIDTH + AGENTS_COLUMN_GAP + AGENTS_LATEST_MIN_WIDTH + metrics_width;
        if body_budget >= mandatory || metrics.is_empty() {
            break;
        }
        metrics.pop();
    }
    let metrics_width: usize = metrics
        .iter()
        .map(|(_, _, label_width)| AGENTS_COLUMN_GAP + label_width)
        .sum();
    let rest = body_budget.saturating_sub(metrics_width);

    let (title, latest) = if rest <= AGENTS_TITLE_MIN_WIDTH {
        // 极窄：只保留 status/title，title 安全截断。
        (truncate_display_width_with_ellipsis(title_text, rest), None)
    } else {
        let title_width = display_width(title_text)
            .min(AGENTS_TITLE_MAX_WIDTH)
            .min(rest - AGENTS_COLUMN_GAP - AGENTS_LATEST_MIN_WIDTH)
            .max(AGENTS_TITLE_MIN_WIDTH);
        let latest_width = rest - title_width - AGENTS_COLUMN_GAP;
        (
            truncate_display_width_with_ellipsis(title_text, title_width),
            (latest_width >= AGENTS_LATEST_MIN_VISIBLE_WIDTH)
                .then(|| truncate_display_width_with_ellipsis(&latest_text, latest_width)),
        )
    };

    let mut layout = AgentsPanelRowLayout {
        status,
        title,
        latest,
        elapsed: None,
        tools: None,
        tokens: None,
    };
    for (kind, label, _) in metrics {
        match kind {
            AgentsMetricKind::Elapsed => layout.elapsed = Some(label),
            AgentsMetricKind::Tools => layout.tools = Some(label),
            AgentsMetricKind::Tokens => layout.tokens = Some(label),
        }
    }
    layout
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
