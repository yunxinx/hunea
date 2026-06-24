use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};

use crate::{
    Model,
    message_history_picker::list_render::MESSAGE_HISTORY_BODY_LEFT_PADDING,
    render_frame::RenderFrame,
    styled_text::render_line_with_full_width_background,
    theme::{build_page_rule, muted_text_style, primary_text_style, tertiary_text_style},
};

impl Model {
    pub(crate) fn render_message_history_picker_preview(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        let Some(preview_scroll_offset) = self
            .message_history_picker
            .as_ref()
            .and_then(|state| state.preview.as_ref())
            .map(|preview| preview.scroll_offset)
        else {
            return;
        };
        let Some(wrapped_lines) = self.message_history_picker_preview_wrapped_lines() else {
            return;
        };
        if area.width == 0 || area.height == 0 {
            return;
        }
        frame.render_widget(Clear, area);
        let palette = self.palette;
        let content_height = usize::from(area.height.saturating_sub(2).max(1));
        let text_style = primary_text_style(palette);
        let page_size = content_height.max(1);
        let max_offset = wrapped_lines.len().saturating_sub(page_size);
        let scroll_offset = preview_scroll_offset.min(max_offset);
        let (page_number, page_count) =
            crate::transcript_overlay::render::transcript_overlay_page_progress(
                wrapped_lines.len(),
                content_height,
                scroll_offset,
            );

        let content_bottom = area
            .y
            .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
        let mut row = area.y;
        for line in wrapped_lines
            .iter()
            .skip(scroll_offset)
            .take(content_height)
        {
            if row >= content_bottom {
                break;
            }
            render_line_with_full_width_background(
                &Line::from(vec![
                    Span::raw(MESSAGE_HISTORY_BODY_LEFT_PADDING),
                    Span::styled(line.as_str(), text_style),
                ]),
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

        if area.height >= 2 {
            let rule_y = area.y + area.height - 2;
            frame.render_widget(
                Paragraph::new(build_page_rule(
                    area.width,
                    page_number,
                    page_count,
                    palette,
                )),
                Rect::new(area.x, rule_y, area.width, 1),
            );
        }
        let footer_y = area.y + area.height - 1;
        frame.render_widget(
            Paragraph::new(Line::styled(
                message_history_picker_preview_footer_hint(area.width).to_string(),
                tertiary_text_style(palette).add_modifier(Modifier::ITALIC),
            )),
            Rect::new(area.x, footer_y, area.width, 1),
        );
    }
}

fn message_history_picker_preview_footer_hint(width: u16) -> &'static str {
    if width < 90 {
        "  Esc back · Space back · c copy · h/l page"
    } else {
        "  Esc back to message list · Space back · c copy · ←/→/h/l page"
    }
}
