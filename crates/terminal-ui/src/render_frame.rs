use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::Widget,
};

/// `RenderFrame` 是 TUI 渲染树写入屏幕缓冲区的统一入口。
pub(crate) struct RenderFrame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor_position: Option<Position>,
}

impl<'a> RenderFrame<'a> {
    pub(crate) fn new(area: Rect, buffer: &'a mut Buffer) -> Self {
        Self {
            area,
            buffer,
            cursor_position: None,
        }
    }

    pub(crate) const fn area(&self) -> Rect {
        self.area
    }

    pub(crate) fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub(crate) fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }

    pub(crate) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }

    pub(crate) const fn cursor_position(&self) -> Option<Position> {
        self.cursor_position
    }
}
