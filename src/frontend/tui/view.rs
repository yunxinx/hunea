use ratatui::{Frame, buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

use super::Model;

struct DocumentViewportWidget<'a> {
    lines: &'a [Line<'static>],
}

impl Widget for DocumentViewportWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            buf.set_line(area.x, y, line, area.width);
        }
    }
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
