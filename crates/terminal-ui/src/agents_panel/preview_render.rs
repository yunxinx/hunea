use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    Model,
    agents_panel::AgentsPanelSurface,
    render_frame::RenderFrame,
    styled_text::render_line_with_full_width_background,
    theme::{
        build_page_rule, muted_text_style, primary_text_style, secondary_text_style,
        tertiary_text_style,
    },
};

use super::preview::{AGENTS_PREVIEW_HORIZONTAL_PADDING, agents_panel_preview_header};

impl Model {
    pub(crate) fn render_agents_panel_preview(&mut self, frame: &mut RenderFrame<'_>, area: Rect) {
        if area.width == 0 || area.height < 4 {
            // header + 至少 1 行正文 + rule + footer。
            return;
        }
        frame.render_widget(Clear, area);
        let palette = self.palette;

        let Some((agent_id, scroll_offset)) = self.agents_panel_preview_surface_position() else {
            return;
        };

        // header 数据优先用 view snapshot（新鲜）；loading 期回退 overview row。
        let header = self.agents_panel_preview_header_data(agent_id, usize::from(area.width));
        let Some(header) = header else {
            return;
        };
        let mut header_spans = vec![
            Span::raw(" ".repeat(AGENTS_PREVIEW_HORIZONTAL_PADDING)),
            Span::styled(header.status, secondary_text_style(palette)),
        ];
        if !header.title.is_empty() {
            header_spans.push(Span::raw(" "));
            header_spans.push(Span::styled(
                header.title,
                primary_text_style(palette).bold(),
            ));
        }
        if let Some(elapsed) = header.elapsed {
            header_spans.push(Span::raw(" "));
            header_spans.push(Span::styled(elapsed, tertiary_text_style(palette)));
        }
        frame.render_widget(
            Paragraph::new(Line::from(header_spans)),
            Rect::new(area.x, area.y, area.width, 1),
        );

        let Some(display_lines) = self.agents_panel_preview_display_lines() else {
            return;
        };
        let content_height = usize::from(area.height.saturating_sub(3).max(1));
        let page_size = content_height.max(1);
        let max_offset = display_lines.len().saturating_sub(page_size);
        let scroll_offset = scroll_offset.min(max_offset);
        let (page_number, page_count) =
            crate::transcript_overlay::render::transcript_overlay_page_progress(
                display_lines.len(),
                content_height,
                scroll_offset,
            );

        let content_bottom = area
            .y
            .saturating_add(1)
            .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
        let mut row = area.y.saturating_add(1);
        let text_style = primary_text_style(palette);
        for line in display_lines
            .iter()
            .skip(scroll_offset)
            .take(content_height)
        {
            if row >= content_bottom {
                break;
            }
            render_line_with_full_width_background(
                &Line::from(Span::styled(line.as_str(), text_style)),
                Rect::new(area.x, row, area.width, 1),
                frame.buffer_mut(),
            );
            row = row.saturating_add(1);
        }

        let fill_style = muted_text_style(palette);
        while row < content_bottom {
            frame.render_widget(
                Paragraph::new(Line::styled("~", fill_style)),
                Rect::new(area.x, row, area.width, 1),
            );
            row = row.saturating_add(1);
        }

        frame.render_widget(
            Paragraph::new(build_page_rule(
                area.width,
                page_number,
                page_count,
                palette,
            )),
            Rect::new(area.x, area.y + area.height - 2, area.width, 1),
        );
        frame.render_widget(
            Paragraph::new(Line::styled(
                agents_panel_preview_footer_hint(area.width),
                tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
            )),
            Rect::new(area.x, area.y + area.height - 1, area.width, 1),
        );
    }

    fn agents_panel_preview_surface_position(
        &self,
    ) -> Option<(runtime_domain::agent::AgentId, usize)> {
        let panel = self.agents_panel.as_ref()?;
        match panel.surface.as_ref()? {
            AgentsPanelSurface::Preview {
                agent_id,
                scroll_offset,
            } => Some((*agent_id, *scroll_offset)),
            _ => None,
        }
    }

    /// preview header 数据：view snapshot 优先，loading 期回退 overview row，
    /// 让 header 在快照到达前就有 status/title 可读。
    fn agents_panel_preview_header_data(
        &self,
        agent_id: runtime_domain::agent::AgentId,
        width: usize,
    ) -> Option<super::preview::AgentsPanelPreviewHeader> {
        let panel = self.agents_panel.as_ref()?;
        let record = panel.agent_view_for_agent(agent_id)?;
        if let Some(preview) = record.snapshot.as_ref().map(|snapshot| &snapshot.preview) {
            return Some(agents_panel_preview_header(
                preview.status,
                preview.title.as_str(),
                preview.elapsed_ms,
                width,
            ));
        }
        let row = panel
            .list
            .rows()
            .iter()
            .find(|row| row.agent_id == agent_id)?;
        Some(agents_panel_preview_header(
            row.status,
            row.title.as_str(),
            row.elapsed_ms,
            width,
        ))
    }
}

fn agents_panel_preview_footer_hint(width: u16) -> &'static str {
    if width < 90 {
        "  Esc back · Space back · ←/→/h/l page"
    } else {
        "  Esc back to agents overview · Space back · ←/→/h/l page"
    }
}
