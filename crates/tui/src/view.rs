use ratatui::{Frame, buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

use super::{
    Model, message::assistant_message_visual_inset,
    styled_text::render_line_with_full_width_background,
};

struct DocumentViewportWidget<'a> {
    lines: &'a [Line<'static>],
    assistant_lines: &'a [bool],
}

impl Widget for DocumentViewportWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        for (row, line) in self.lines.iter().take(usize::from(area.height)).enumerate() {
            let y = area.y + u16::try_from(row).unwrap_or(u16::MAX);
            if self.assistant_lines.get(row).copied().unwrap_or(false) {
                render_inset_line(line, area, y, buf);
            } else {
                render_line_with_full_width_background(
                    line,
                    Rect::new(area.x, y, area.width, 1),
                    buf,
                );
            }
        }
    }
}

fn render_inset_line(line: &Line<'static>, area: Rect, y: u16, buf: &mut Buffer) {
    let inset = assistant_message_visual_inset(area.width);
    if inset == 0 || area.width <= inset.saturating_mul(2) {
        render_line_with_full_width_background(line, Rect::new(area.x, y, area.width, 1), buf);
        return;
    }

    buf.set_line(area.x, y, &Line::raw(""), area.width);
    render_line_with_full_width_background(
        line,
        Rect::new(
            area.x + inset,
            y,
            area.width.saturating_sub(inset.saturating_mul(2)),
            1,
        ),
        buf,
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

    // Transcript 覆盖层模式：全屏渲染对话历史，隐藏 composer 和各面板
    if model.transcript_overlay_active() {
        model.render_transcript_overlay(frame, area);
        return;
    }

    let document = model.build_document_layout();
    let viewport = model.build_document_viewport(&document);

    frame.render_widget(
        DocumentViewportWidget {
            lines: &viewport.lines,
            assistant_lines: &viewport.assistant_lines,
        },
        area,
    );

    if model.history_scroll_indicator_visible() {
        model.render_history_scroll_indicator(frame, area, &document, &viewport);
    }

    if model.has_current_floating_layer() {
        let floating_layer = model.current_floating_layer(&document, &viewport);
        frame.render_widget(floating_layer, area);
    }

    if let Some(cursor_y) = document.cursor_y.checked_sub(viewport.resolved_offset)
        && cursor_y < viewport.lines.len()
    {
        frame.set_cursor_position((
            area.x + document.cursor_x,
            area.y + u16::try_from(cursor_y).unwrap_or(u16::MAX),
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::{Terminal, backend::TestBackend, layout::Position, style::Color};

    use super::*;
    use crate::{HeroOptions, ReasoningDisplayMode, StyleMode, theme::default_palette};
    use ::mo_acp::{AcpToolCall, AcpToolCallContent, AcpToolCallStatus, AcpToolKind};

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
    fn render_hides_cursor_when_composer_cursor_is_above_viewport() {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.transcript_mut().clear();
        model.set_window(20, 4);
        model.set_palette(default_palette(), true);
        model
            .composer_mut()
            .set_text_for_test("line one\nline two\nline three\nline four\nline five");
        model.composer_mut().move_to_begin_for_test();
        model.sync_composer_height();

        let layout = model.build_document_layout();
        let document_offset = layout.cursor_y + 1;
        let composer_offset = model.current_composer_viewport_offset(&layout, document_offset);
        model.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            false,
            true,
        );

        let mut terminal = Terminal::new(TestBackend::new(20, 4)).unwrap();
        let sentinel = Position::new(17, 3);
        terminal.set_cursor_position(sentinel).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();

        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            sentinel,
            "render must not pin the hidden composer cursor to viewport row 0"
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

    #[test]
    fn diff_line_background_fills_the_rendered_row() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.set_window(48, 8);
        model.set_palette(default_palette(), true);
        model.append_acp_tool_call_from_runtime(AcpToolCall {
            tool_call_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
            kind: AcpToolKind::Edit,
            status: AcpToolCallStatus::Completed,
            content: vec![AcpToolCallContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("one\nold\ntail\n".to_string()),
                new_text: "one\nnew\ntail\n".to_string(),
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        });

        let mut terminal = Terminal::new(TestBackend::new(48, 8)).unwrap();
        terminal.draw(|frame| model.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows = rendered_rows(buffer);
        let insert_row = rows
            .iter()
            .position(|row| row.contains("+  new"))
            .expect("insert diff row should be rendered");

        assert_ne!(
            buffer[(47, u16::try_from(insert_row).unwrap())].bg,
            Color::Reset,
            "diff insert row background should fill trailing cells: {rows:?}"
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
