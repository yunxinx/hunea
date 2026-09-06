use ratatui::layout::Rect;

use crate::{Model, render_frame::RenderFrame};

impl Model {
    /// `/agents` panel 的顶层渲染分派：list / preview / transcript 三态共用一个
    /// `ModalLayer::AgentsOverview`（层内子模式，对齐 entry_tree 的多层先例）。
    pub(crate) fn render_agents_panel(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if self.agents_panel_preview_active() {
            self.render_agents_panel_preview(frame, area);
        } else if self.agents_panel_transcript_active() {
            self.render_agents_panel_transcript(frame, area);
        } else {
            self.render_agents_panel_list(frame, area);
        }
    }
}
