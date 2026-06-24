use ratatui::layout::Rect;

use crate::{Model, render_frame::RenderFrame};

impl Model {
    pub(crate) fn render_message_history_picker(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        if self.message_history_picker_preview_active() {
            self.render_message_history_picker_preview(frame, area);
        } else {
            self.render_message_history_picker_list(frame, area);
        }
    }
}
