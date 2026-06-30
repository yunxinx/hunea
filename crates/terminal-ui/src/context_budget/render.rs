use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
};

use super::{
    CONTEXT_BUDGET_PANEL_INSET_WIDTH, CONTEXT_BUDGET_SECTION_GAP_ROWS,
    heatmap::build_context_budget_heatmap_lines, layout::context_budget_body_layout,
    legend::build_context_budget_legend_lines, state::context_budget_error_message,
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
    let mut lines = vec![
        inline_panel_rule_line(terminal_width, model.palette),
        context_budget_header_line(model, terminal_width),
    ];
    append_blank_lines(&mut lines, CONTEXT_BUDGET_SECTION_GAP_ROWS);
    let footer_lines = context_budget_footer_lines(model);
    let body_height = visible_rows
        .saturating_sub(lines.len() + CONTEXT_BUDGET_SECTION_GAP_ROWS + footer_lines.len())
        .max(1);
    lines.extend(context_budget_body_lines(
        model,
        terminal_width,
        body_height,
    ));
    append_blank_lines(&mut lines, CONTEXT_BUDGET_SECTION_GAP_ROWS);
    lines.extend(footer_lines);
    lines
}

fn context_budget_header_line(model: &Model, width: usize) -> Line<'static> {
    let inset = usize::from(CONTEXT_BUDGET_PANEL_INSET_WIDTH);
    Line::from(vec![
        Span::raw(panel_inset_text()),
        Span::styled(
            truncate_display_width_with_ellipsis(
                "Context Usage",
                width.saturating_sub(inset).max(1),
            ),
            primary_text_style(model.palette).bold(),
        ),
    ])
}

fn context_budget_footer_lines(model: &Model) -> [Line<'static>; 1] {
    [Line::styled(
        format!("{}Esc close", panel_inset_text()),
        tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC),
    )]
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
                format!("{}Loading context budget...", panel_inset_text()),
                tertiary_text_style(model.palette),
            )],
            body_height,
        );
    }

    if let Some(error) = state.error.as_ref() {
        let message = context_budget_error_message(error);
        return pad_body_lines(
            vec![Line::styled(
                truncate_display_width_with_ellipsis(
                    &format!("{}{message}", panel_inset_text()),
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
                format!(
                    "{}Terminal too small for context budget",
                    panel_inset_text()
                ),
                tertiary_text_style(palette),
            )],
            usize::from(area.height),
        );
    };
    let left_padding = usize::from(layout.content.x.saturating_sub(area.x));
    let row_count = usize::from(layout.content.height);
    let heatmap_lines = build_context_budget_heatmap_lines(layout.heatmap, snapshot, palette);
    let legend_lines = build_context_budget_legend_lines(layout.legend, snapshot, palette);

    (0..row_count)
        .map(|row| {
            let mut spans = Vec::new();
            if left_padding > 0 {
                spans.push(Span::raw(" ".repeat(left_padding)));
            }
            spans.extend(
                heatmap_lines
                    .get(row)
                    .cloned()
                    .unwrap_or_else(|| blank_line(usize::from(layout.heatmap.width)))
                    .spans,
            );
            if layout.legend.x > layout.heatmap.x + layout.heatmap.width {
                spans.push(Span::raw(" ".repeat(usize::from(
                    layout.legend.x - (layout.heatmap.x + layout.heatmap.width),
                ))));
            }
            spans.extend(
                legend_lines
                    .get(row)
                    .cloned()
                    .unwrap_or_else(|| blank_line(usize::from(layout.legend.width)))
                    .spans,
            );
            Line::from(spans)
        })
        .collect()
}

fn pad_body_lines(mut lines: Vec<Line<'static>>, body_height: usize) -> Vec<Line<'static>> {
    lines.resize(body_height.max(lines.len()), Line::raw(""));
    lines
}

fn blank_line(width: usize) -> Line<'static> {
    if width == 0 {
        Line::raw("")
    } else {
        Line::raw(" ".repeat(width))
    }
}

fn append_blank_lines(lines: &mut Vec<Line<'static>>, count: usize) {
    for _ in 0..count {
        lines.push(Line::raw(""));
    }
}

fn panel_inset_text() -> String {
    " ".repeat(usize::from(CONTEXT_BUDGET_PANEL_INSET_WIDTH))
}

#[cfg(test)]
mod tests {
    use crate::context_budget::summary::context_usage_summary;
    use runtime_domain::{context_budget::ContextTokenLimit, session::ContextWindowUsagePayload};

    fn limit(value: u32) -> ContextTokenLimit {
        ContextTokenLimit::try_from(value).expect("fixture limit should be valid")
    }

    #[test]
    fn usage_summary_absolute_shows_documented_display_limit() {
        let text = context_usage_summary(
            "qwen3",
            ContextWindowUsagePayload {
                limit: limit(256_000),
                used: 42_000,
                percent: 16.4,
                is_saturated: false,
            },
        );
        assert_eq!(text, "qwen3 · 42k/256k tokens (16.4%)");
    }

    #[test]
    fn usage_summary_absolute_shows_limit_without_percent() {
        let text = context_usage_summary(
            "gpt-4o",
            ContextWindowUsagePayload {
                limit: limit(128_000),
                used: 32_000,
                percent: 25.0,
                is_saturated: false,
            },
        );
        assert!(text.contains("128k"));
        assert!(text.contains("gpt-4o"));
        assert!(text.contains("tokens"));
        assert!(text.contains("(25%)"));
    }
}
