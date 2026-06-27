use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::{
    heatmap::render_context_budget_heatmap, layout::context_budget_body_layout,
    legend::render_context_budget_legend, state::header_summary,
};
use crate::{
    Model,
    inline_panel::{
        InlinePanelRenderResult, inline_panel_render_result, inline_panel_rule_line,
        inline_panel_visible_rows,
    },
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, primary_text_style, tertiary_text_style},
};

const CONTEXT_BUDGET_PANEL_VISIBLE_ROWS: usize = 15;
const CONTEXT_BUDGET_MIN_PANEL_WIDTH: usize = 45;

pub(crate) type ContextBudgetRenderResult = InlinePanelRenderResult;

impl Model {
    pub(crate) fn current_inline_context_budget_render_result(&self) -> InlinePanelRenderResult {
        if !self.context_budget_active() {
            return InlinePanelRenderResult::default();
        }

        let visible_rows = inline_panel_visible_rows(self, CONTEXT_BUDGET_PANEL_VISIBLE_ROWS);
        let terminal_width = usize::from(self.width.max(1));
        let mut lines = build_panel_lines(self, terminal_width, visible_rows);
        if lines.len() > visible_rows {
            lines.truncate(visible_rows);
        }
        inline_panel_render_result(lines)
    }
}

fn build_panel_lines(
    model: &Model,
    terminal_width: usize,
    visible_rows: usize,
) -> Vec<Line<'static>> {
    let terminal_width = terminal_width.max(1);
    let content_width = context_budget_panel_width(terminal_width);
    let mut lines = vec![
        inline_panel_rule_line(terminal_width, model.palette),
        context_budget_header_line(model, terminal_width),
        Line::raw(""),
    ];
    let footer_lines = context_budget_footer_lines(model);
    let body_height = visible_rows
        .saturating_sub(lines.len() + footer_lines.len())
        .max(1);
    lines.extend(context_budget_body_lines(model, content_width, body_height));
    lines.extend(footer_lines);
    lines
}

fn context_budget_panel_width(total_width: usize) -> usize {
    if total_width <= CONTEXT_BUDGET_MIN_PANEL_WIDTH {
        return total_width.max(1);
    }

    total_width
        .saturating_mul(3)
        .div_ceil(5)
        .max(CONTEXT_BUDGET_MIN_PANEL_WIDTH)
        .min(total_width)
}

fn context_budget_header_line(model: &Model, width: usize) -> Line<'static> {
    let text = model
        .context_budget
        .as_ref()
        .map(context_budget_header_text)
        .unwrap_or_else(|| "Context Usage".to_string());
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            truncate_display_width_with_ellipsis(&text, width.saturating_sub(2).max(1)),
            primary_text_style(model.palette).bold(),
        ),
    ])
}

fn context_budget_header_text(state: &super::state::ContextBudgetState) -> String {
    if let Some(snapshot) = state.snapshot.as_ref() {
        header_summary(&snapshot.model_id, snapshot.display)
    } else if state.loading {
        "Context Usage · loading…".to_string()
    } else if let Some(error) = state.error.as_ref() {
        format!("Context Usage · {error}")
    } else {
        "Context Usage".to_string()
    }
}

fn context_budget_footer_lines(model: &Model) -> [Line<'static>; 2] {
    [
        Line::raw(""),
        Line::styled(
            "  Esc close",
            tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
        ),
    ]
}

fn context_budget_body_lines(
    model: &Model,
    width: usize,
    body_height: usize,
) -> Vec<Line<'static>> {
    let area = Rect::new(
        0,
        0,
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(body_height).unwrap_or(u16::MAX),
    );
    if area.is_empty() {
        return Vec::new();
    }

    let Some(state) = model.context_budget.as_ref() else {
        return vec![Line::raw("")];
    };

    if let Some(snapshot) = state.snapshot.as_ref() {
        return context_budget_snapshot_body_lines(area, snapshot, model.palette);
    }

    if state.loading {
        return pad_body_lines(
            vec![Line::styled(
                "  Loading context budget...",
                tertiary_text_style(model.palette),
            )],
            body_height,
        );
    }

    if let Some(error) = state.error.as_ref() {
        return pad_body_lines(
            vec![Line::styled(
                truncate_display_width_with_ellipsis(
                    &format!("  {error}"),
                    usize::from(area.width).max(1),
                ),
                tertiary_text_style(model.palette),
            )],
            body_height,
        );
    }

    pad_body_lines(vec![Line::raw("")], body_height)
}

fn context_budget_snapshot_body_lines(
    area: Rect,
    snapshot: &runtime_domain::session::ContextBudgetSnapshotPayload,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let Some(layout) = context_budget_body_layout(area) else {
        return pad_body_lines(
            vec![Line::styled(
                "  Terminal too small for context budget",
                tertiary_text_style(palette),
            )],
            usize::from(area.height),
        );
    };

    let mut buffer = Buffer::empty(area);
    render_context_budget_heatmap(&mut buffer, layout.heatmap, snapshot, palette);
    render_context_budget_legend(&mut buffer, layout.legend, snapshot, palette);
    buffer_rows_to_lines(&buffer, area)
}

fn buffer_rows_to_lines(buffer: &Buffer, area: Rect) -> Vec<Line<'static>> {
    (0..area.height)
        .map(|row| buffer_row_to_line(buffer, area, area.y + row))
        .collect()
}

fn buffer_row_to_line(buffer: &Buffer, area: Rect, y: u16) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current_style: Option<Style> = None;
    let mut current_text = String::new();

    for x in area.x..area.x + area.width {
        let cell = &buffer[(x, y)];
        let style = Style::default()
            .fg(cell.fg)
            .bg(cell.bg)
            .add_modifier(cell.modifier);
        if current_style == Some(style) {
            current_text.push_str(cell.symbol());
            continue;
        }

        if let Some(style) = current_style {
            spans.push(Span::styled(std::mem::take(&mut current_text), style));
        } else if !current_text.is_empty() {
            spans.push(Span::raw(std::mem::take(&mut current_text)));
        }

        current_style = Some(style);
        current_text.push_str(cell.symbol());
    }

    if let Some(style) = current_style {
        spans.push(Span::styled(current_text, style));
    } else if !current_text.is_empty() {
        spans.push(Span::raw(current_text));
    }

    Line::from(spans)
}

fn pad_body_lines(mut lines: Vec<Line<'static>>, body_height: usize) -> Vec<Line<'static>> {
    lines.resize(body_height.max(lines.len()), Line::raw(""));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_domain::session::ContextBudgetDisplayPayload;

    #[test]
    fn header_relative_shows_question_mark_limit() {
        let text = header_summary(
            "qwen3",
            ContextBudgetDisplayPayload::Relative { used: 42_000 },
        );
        assert!(text.contains("qwen3"));
        assert!(text.contains("/ ?"));
    }

    #[test]
    fn header_absolute_shows_limit_and_percent() {
        let text = header_summary(
            "gpt-4o",
            ContextBudgetDisplayPayload::Absolute {
                limit: 128_000,
                used: 32_000,
                percent: 25.0,
            },
        );
        assert!(text.contains("128k"));
        assert!(text.contains("25.0%"));
    }

    #[test]
    fn panel_width_uses_about_sixty_percent_when_terminal_is_wide_enough() {
        assert_eq!(context_budget_panel_width(72), 45);
        assert_eq!(context_budget_panel_width(80), 48);
    }
}
