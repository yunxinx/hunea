use std::time::{Duration, Instant};

use crossterm::event::MouseButton;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{
    AppEffect, Model,
    document::{DocumentLayout, DocumentViewport},
    status_line::truncate_display_width,
    theme::tertiary_text_style,
};

pub(crate) const HISTORY_SCROLL_INDICATOR_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HistoryScrollIndicatorBounds {
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) width: u16,
}

impl Model {
    pub(crate) fn show_history_scroll_indicator(&mut self) {
        if self.follow_bottom || !self.manual_document_scroll {
            self.clear_history_scroll_indicator();
            return;
        }

        self.history_scroll_indicator_token += 1;
        self.history_scroll_indicator_deadline =
            Some(Instant::now() + HISTORY_SCROLL_INDICATOR_WINDOW);
    }

    pub(crate) fn dismiss_history_scroll_indicator(&mut self, token: usize) {
        if token != self.history_scroll_indicator_token {
            return;
        }

        self.clear_history_scroll_indicator();
    }

    pub(crate) fn clear_history_scroll_indicator(&mut self) {
        self.history_scroll_indicator_deadline = None;
    }

    pub(crate) fn history_scroll_indicator_visible(&self) -> bool {
        if self.follow_bottom || !self.manual_document_scroll {
            return false;
        }

        self.history_scroll_indicator_deadline
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub(crate) fn render_history_scroll_indicator(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        layout: &DocumentLayout,
        viewport: &DocumentViewport,
    ) {
        let Some((line, bounds)) = self.current_history_scroll_indicator_line(layout, viewport)
        else {
            return;
        };

        frame.render_widget(
            ratatui::widgets::Paragraph::new(line),
            Rect::new(area.x + bounds.column, area.y + bounds.row, bounds.width, 1),
        );
    }

    pub(crate) fn handle_history_scroll_indicator_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> Option<Option<AppEffect>> {
        if !self.history_scroll_indicator_hit(column, row) {
            return None;
        }

        self.cancel_exit_confirmation();
        self.clear_history_scroll_indicator();
        match button {
            MouseButton::Middle => {
                self.clear_pending_composer_cursor_click();
                self.reset_selection_click();
                Some(self.request_copy_selection())
            }
            MouseButton::Left => {
                self.stop_selection_auto_scroll();
                self.clear_pending_composer_cursor_click();
                self.clear_selection();
                Some(None)
            }
            _ => Some(None),
        }
    }

    fn history_scroll_indicator_hit(&mut self, column: u16, row: u16) -> bool {
        if !self.history_scroll_indicator_visible() {
            return false;
        }

        let layout = self.build_document_layout();
        let viewport = self.build_document_viewport(&layout);
        let Some(bounds) = self.current_history_scroll_indicator_bounds(&layout, &viewport) else {
            return false;
        };

        row == bounds.row
            && column >= bounds.column
            && column < bounds.column.saturating_add(bounds.width)
    }

    fn current_history_scroll_indicator_line(
        &self,
        layout: &DocumentLayout,
        viewport: &DocumentViewport,
    ) -> Option<(Line<'static>, HistoryScrollIndicatorBounds)> {
        let text = self.history_scroll_indicator_text(layout)?;
        let bounds = self.current_history_scroll_indicator_bounds(layout, viewport)?;
        let visible_text = truncate_display_width(&text, usize::from(bounds.width));
        let mut style = tertiary_text_style(self.palette);
        if let Some(surface) = self.palette.surface {
            style = style.bg(surface);
        }

        Some((Line::from(vec![Span::styled(visible_text, style)]), bounds))
    }

    fn current_history_scroll_indicator_bounds(
        &self,
        layout: &DocumentLayout,
        viewport: &DocumentViewport,
    ) -> Option<HistoryScrollIndicatorBounds> {
        let text = self.history_scroll_indicator_text(layout)?;
        if self.width == 0 || viewport.lines.is_empty() {
            return None;
        }

        let width = text.width().min(usize::from(self.width.max(1)));
        if width == 0 {
            return None;
        }

        let row = viewport.lines.len().saturating_sub(1).min(1);
        Some(HistoryScrollIndicatorBounds {
            column: self
                .width
                .saturating_sub(u16::try_from(width).unwrap_or(u16::MAX)),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            width: u16::try_from(width).unwrap_or(u16::MAX),
        })
    }

    fn history_scroll_indicator_text(&self, layout: &DocumentLayout) -> Option<String> {
        if !self.history_scroll_indicator_visible() {
            return None;
        }

        let percentage = self.history_scroll_percentage(layout)?;
        Some(format!("{percentage} %"))
    }

    fn history_scroll_percentage(&self, layout: &DocumentLayout) -> Option<usize> {
        if layout.transcript_line_count == 0 {
            return None;
        }

        let mut top_transcript_line = None;
        let mut visible_transcript_lines = 0usize;
        for line_index in self.document_viewport_line_indices(layout) {
            if line_index >= layout.transcript_line_count {
                continue;
            }
            top_transcript_line.get_or_insert(line_index);
            visible_transcript_lines += 1;
        }

        let top_transcript_line = top_transcript_line?;
        if visible_transcript_lines == 0 {
            return None;
        }

        let max_top_line = layout
            .transcript_line_count
            .saturating_sub(visible_transcript_lines);
        if max_top_line == 0 {
            return Some(0);
        }

        Some(((top_transcript_line * 100 + max_top_line / 2) / max_top_line).clamp(0, 100))
    }
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use crate::frontend::tui::{
        HeroOptions, Model, Sender,
        document::{DocumentLayout, DocumentViewport},
    };

    #[test]
    fn transcript_fully_visible_reports_zero_percent() {
        let mut model = Model::new(HeroOptions::default());
        model.width = 20;
        model.height = 5;
        model.has_window = true;
        model.follow_bottom = false;
        model.manual_document_scroll = true;
        model.document_viewport_y = 0;
        model.show_history_scroll_indicator();

        let layout = DocumentLayout {
            transcript_line_count: 5,
            lines: vec![
                Line::raw("a"),
                Line::raw("b"),
                Line::raw("c"),
                Line::raw("d"),
                Line::raw("e"),
            ],
            plain_lines: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
            ],
            ..DocumentLayout::default()
        };

        assert_eq!(model.history_scroll_percentage(&layout), Some(0));
    }

    #[test]
    fn stale_timeout_token_does_not_hide_indicator() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(20, 3);
        model.transcript_mut().clear();
        model
            .transcript_mut()
            .append_message(Sender::Assistant, "a\nb\nc\nd\ne");
        model.sync_transcript_render();
        model.composer.replace_text_and_move_to_end("x");
        model.sync_composer_height();
        model.sync_document_viewport_to_bottom();
        model.scroll_document_by(-3);
        model.show_history_scroll_indicator();
        let token = model.history_scroll_indicator_token;

        model.dismiss_history_scroll_indicator(token.saturating_sub(1));

        assert!(model.history_scroll_indicator_visible());
        model.dismiss_history_scroll_indicator(token);
        assert!(!model.history_scroll_indicator_visible());
    }

    #[test]
    fn single_line_viewport_places_indicator_on_first_row() {
        let mut model = Model::new(HeroOptions::default());
        model.width = 20;
        model.height = 1;
        model.has_window = true;
        model.follow_bottom = false;
        model.manual_document_scroll = true;
        model.document_viewport_y = 2;
        model.show_history_scroll_indicator();

        let layout = DocumentLayout {
            transcript_line_count: 5,
            lines: vec![
                Line::raw("a"),
                Line::raw("b"),
                Line::raw("c"),
                Line::raw("d"),
                Line::raw("e"),
            ],
            plain_lines: vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
                "e".to_string(),
            ],
            ..DocumentLayout::default()
        };
        let viewport = DocumentViewport {
            lines: vec![Line::raw("c")],
            plain_lines: vec!["c".to_string()],
            resolved_offset: 2,
        };
        let bounds = model
            .current_history_scroll_indicator_bounds(&layout, &viewport)
            .expect("indicator bounds should exist");

        assert_eq!(bounds.row, 0);
    }
}
