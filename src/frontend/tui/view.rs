use ratatui::{Frame, buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

use super::{Model, document::DocumentLayout, message::assistant_message_visual_inset};

struct DocumentViewportWidget<'a> {
    lines: &'a [Line<'static>],
    layout: &'a DocumentLayout,
    resolved_offset: usize,
}

impl Widget for DocumentViewportWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            let line_index = self.resolved_offset + row;
            if self.layout.is_assistant_message_line(line_index) {
                render_inset_line(line, area, y, buf);
            } else {
                buf.set_line(area.x, y, line, area.width);
            }
        }
    }
}

fn render_inset_line(line: &Line<'static>, area: Rect, y: u16, buf: &mut Buffer) {
    let inset = assistant_message_visual_inset(area.width);
    if inset == 0 || area.width <= inset.saturating_mul(2) {
        buf.set_line(area.x, y, line, area.width);
        return;
    }

    buf.set_line(area.x, y, &Line::raw(""), area.width);
    buf.set_line(
        area.x + inset,
        y,
        line,
        area.width.saturating_sub(inset.saturating_mul(2)),
    );
}

/// `render` 负责将统一文档流映射到当前帧内容。
pub fn render(model: &mut Model, frame: &mut Frame<'_>) {
    if !model.is_ready() {
        return;
    }

    let area = frame.area();
    if area.is_empty() {
        return;
    }

    let document = model.build_document_layout();
    let viewport = model.build_document_viewport(&document);

    frame.render_widget(
        DocumentViewportWidget {
            lines: &viewport.lines,
            layout: &document,
            resolved_offset: viewport.resolved_offset,
        },
        area,
    );
    model.render_history_scroll_indicator(frame, area, &document, &viewport);

    let cursor_y = document.cursor_y.saturating_sub(viewport.resolved_offset);
    if cursor_y < viewport.lines.len() {
        frame.set_cursor_position((
            area.x + document.cursor_x,
            area.y + u16::try_from(cursor_y).unwrap_or(u16::MAX),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::frontend::tui::{
        HeroOptions, ReasoningDisplayMode, StyleMode, theme::default_palette,
    };

    #[test]
    fn assistant_message_uses_two_column_visual_inset() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.append_assistant_message_from_runtime("hello world");

        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        assert!(
            rendered_rows(buffer)
                .iter()
                .any(|row| row == "  hello world       "),
            "assistant row should be rendered with a two-column visual inset: {:?}",
            rendered_rows(buffer)
        );
    }

    #[test]
    fn assistant_visual_inset_does_not_change_viewport_plain_lines() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.append_assistant_message_from_runtime("hello world");

        let layout = model.build_document_layout();
        let viewport = model.build_document_viewport(&layout);

        assert!(
            viewport
                .plain_lines
                .iter()
                .any(|line| line.as_str() == "hello world"),
            "assistant visual inset must not add spaces to viewport plain lines: {:?}",
            viewport.plain_lines
        );
    }

    #[test]
    fn snippet_reasoning_renders_without_assistant_visual_inset() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model
            .transcript_mut()
            .append_assistant_message_with_reasoning(
                "",
                "hidden reasoning",
                ReasoningDisplayMode::Snippet,
                Some(Duration::from_secs(16)),
                StyleMode::Cx,
            );

        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();

        assert!(
            rendered_rows(terminal.backend().buffer())
                .iter()
                .any(|row| row == "• thoughts 16s      "),
            "snippet reasoning should start at column zero without assistant inset: {:?}",
            rendered_rows(terminal.backend().buffer())
        );
    }

    #[test]
    fn assistant_message_wraps_before_visual_inset_clips_content() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.append_assistant_message_from_runtime("abcdefghijklmnopqrstuvwxyz");

        let mut terminal = Terminal::new(TestBackend::new(20, 8)).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();

        let rows = rendered_rows(terminal.backend().buffer());

        assert!(
            rows.iter().any(|row| row == "  abcdefghijklmnop  "),
            "first assistant visual row should fit the inset content width: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "  qrstuvwxyz        "),
            "overflow should wrap to the next assistant row instead of being clipped: {rows:?}"
        );
    }

    fn rendered_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|row| {
                let mut line = String::new();
                for column in 0..buffer.area.width {
                    line.push_str(buffer[(column, row)].symbol());
                }
                line
            })
            .collect()
    }
}
