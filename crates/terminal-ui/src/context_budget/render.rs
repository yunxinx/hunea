use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use super::{
    heatmap::render_context_budget_heatmap, layout::context_budget_body_layout,
    legend::render_context_budget_legend, state::header_summary,
};
use crate::{
    Model,
    fullscreen_list_chrome::fullscreen_list_chrome_rects,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    theme::{build_page_rule, primary_text_style, subtle_rule_line, tertiary_text_style},
};

impl Model {
    pub(crate) fn render_context_budget(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        let Some(state) = self.context_budget.as_ref() else {
            return;
        };

        frame.render_widget(Clear, area);
        let Some(chrome) = fullscreen_list_chrome_rects(area) else {
            return;
        };

        let header_text = if let Some(snapshot) = state.snapshot.as_ref() {
            header_summary(&snapshot.model_id, snapshot.display)
        } else if state.loading {
            "Context budget · loading…".to_string()
        } else if let Some(error) = state.error.as_ref() {
            format!("Context budget · {error}")
        } else {
            "Context budget".to_string()
        };

        frame.render_widget(
            Paragraph::new(context_budget_header_line(
                &header_text,
                usize::from(area.width),
                self.palette,
            )),
            chrome.header,
        );
        frame.render_widget(
            Paragraph::new(subtle_rule_line(usize::from(area.width), self.palette)),
            chrome.header_rule,
        );

        if let Some(snapshot) = state.snapshot.as_ref() {
            if let Some(body_layout) = context_budget_body_layout(chrome.body) {
                render_context_budget_heatmap(
                    frame.buffer_mut(),
                    body_layout.heatmap,
                    snapshot,
                    self.palette,
                );
                frame.render_widget(
                    Paragraph::new(subtle_rule_line(
                        usize::from(body_layout.divider.width),
                        self.palette,
                    )),
                    body_layout.divider,
                );
                render_context_budget_legend(
                    frame.buffer_mut(),
                    body_layout.legend_columns,
                    snapshot,
                    self.palette,
                );
            } else {
                frame.render_widget(
                    Paragraph::new(Line::styled(
                        "  Terminal too small for context budget",
                        tertiary_text_style(self.palette),
                    )),
                    chrome.body,
                );
            }
        } else if state.loading {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    "  Loading context budget...",
                    tertiary_text_style(self.palette),
                )),
                chrome.body,
            );
        } else if let Some(error) = state.error.as_ref() {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    truncate_display_width_with_ellipsis(
                        &format!("  {error}"),
                        usize::from(chrome.body.width).max(1),
                    ),
                    tertiary_text_style(self.palette),
                )),
                chrome.body,
            );
        }

        frame.render_widget(
            Paragraph::new(build_page_rule(area.width, 1, 1, self.palette)),
            chrome.page_rule,
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                "  Esc close",
                tertiary_text_style(self.palette).add_modifier(Modifier::ITALIC),
            )),
            chrome.footer,
        );
    }
}

fn context_budget_header_line(
    text: &str,
    width: usize,
    palette: crate::theme::TerminalPalette,
) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            truncate_display_width_with_ellipsis(text, width.saturating_sub(2).max(1)),
            primary_text_style(palette).bold(),
        ),
    ])
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
}
