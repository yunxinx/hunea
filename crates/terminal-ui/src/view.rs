use std::time::Instant;

use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::Widget};

use super::{
    Model,
    document::{DocumentLayout, DocumentViewport},
    message::assistant_message_visual_inset,
    modal_layer::ModalLayer,
    render_frame::RenderFrame,
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
pub fn render(model: &mut Model, frame: &mut RenderFrame<'_>) {
    if !model.is_ready() {
        return;
    }

    let area = frame.area();
    if area.is_empty() {
        return;
    }

    if render_active_overlay(model, frame, area) {
        model.render_toast(frame, area);
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

    let startup_banner_entrance_area = if model.startup_banner_entrance_target_available() {
        startup_banner_entrance_rect(&document, &viewport, area).unwrap_or_default()
    } else {
        Rect::default()
    };
    model.apply_startup_banner_entrance_at(
        Instant::now(),
        frame.buffer_mut(),
        startup_banner_entrance_area,
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

    model.render_toast(frame, area);
}

fn render_active_overlay(model: &mut Model, frame: &mut RenderFrame<'_>, area: Rect) -> bool {
    let Some(layer) = model.top_modal_layer() else {
        return false;
    };

    model.complete_startup_banner_entrance();
    match layer {
        // 文件审批预览需要完整审查 diff，超出当前屏幕时进入独立全屏界面。
        ModalLayer::ToolApprovalFullscreenPreview => {
            model.render_tool_approval_fullscreen_preview(frame, area);
        }
        // Transcript 覆盖层模式：全屏渲染对话历史，隐藏 composer 和各面板。
        ModalLayer::TranscriptOverlay => model.render_transcript_overlay(frame, area),
        ModalLayer::PromptOverlay => model.render_prompt_overlay(frame, area),
        ModalLayer::SessionPreview => model.render_session_preview(frame, area),
        ModalLayer::SessionPicker => model.render_session_picker(frame, area),
        ModalLayer::CopyPicker => model.render_copy_picker(frame, area),
        ModalLayer::EntryTree => model.render_entry_tree(frame, area),
        ModalLayer::MessageHistory => model.render_message_history_picker(frame, area),
    }
    true
}

fn startup_banner_entrance_rect(
    layout: &DocumentLayout,
    viewport: &DocumentViewport,
    area: Rect,
) -> Option<Rect> {
    let banner_lines = layout.transcript_item_lines(0)?;
    let banner_start = banner_lines.content_start_line();
    let banner_end = banner_start.saturating_add(banner_lines.content_line_count());
    let viewport_start = viewport.resolved_offset;
    let viewport_end = viewport_start.saturating_add(viewport.lines.len());
    let visible_start = banner_start.max(viewport_start);
    let visible_end = banner_end.min(viewport_end);
    if visible_start >= visible_end {
        return None;
    }

    let first_visible_row = visible_start.saturating_sub(viewport_start);
    let visible_height = visible_end.saturating_sub(visible_start);
    let width = viewport.lines[first_visible_row..first_visible_row + visible_height]
        .iter()
        .map(Line::width)
        .max()
        .unwrap_or_default()
        .min(usize::from(area.width));
    if width == 0 || visible_height == 0 {
        return None;
    }

    Some(Rect::new(
        area.x,
        area.y + u16::try_from(first_visible_row).unwrap_or(u16::MAX),
        u16::try_from(width).unwrap_or(u16::MAX),
        u16::try_from(visible_height).unwrap_or(u16::MAX),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::{buffer::Buffer, layout::Rect, style::Color};

    use super::*;
    use crate::{ReasoningDisplayMode, StartupBannerOptions, StyleMode, theme::default_palette};
    use runtime_domain::session::{
        RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
    };

    #[test]
    fn assistant_message_uses_two_column_visual_inset() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.append_assistant_message_from_runtime("hello world");

        let buffer = render_model_buffer(&mut model, 20, 8);
        assert!(
            rendered_rows(&buffer)
                .iter()
                .any(|row| row == "  hello world       "),
            "assistant row should be rendered with a two-column visual inset: {:?}",
            rendered_rows(&buffer)
        );
    }

    #[test]
    fn assistant_visual_inset_does_not_change_viewport_plain_lines() {
        let mut model = Model::new(StartupBannerOptions::default());
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
        let mut model = Model::new(StartupBannerOptions::default());
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

        let buffer = render_model_buffer(&mut model, 20, 8);

        assert!(
            rendered_rows(&buffer)
                .iter()
                .any(|row| row == "• thoughts 16s      "),
            "snippet reasoning should start at column zero without assistant inset: {:?}",
            rendered_rows(&buffer)
        );
    }

    #[test]
    fn render_hides_cursor_when_composer_cursor_is_above_viewport() {
        let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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

        let area = Rect::new(0, 0, 20, 4);
        let mut buffer = Buffer::empty(area);
        let cursor_position = model.render_to_buffer(area, &mut buffer);

        assert_eq!(
            cursor_position, None,
            "render must not expose the hidden composer cursor"
        );
    }

    #[test]
    fn assistant_message_wraps_before_visual_inset_clips_content() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.append_assistant_message_from_runtime("abcdefghijklmnopqrstuvwxyz");

        let buffer = render_model_buffer(&mut model, 20, 8);
        let rows = rendered_rows(&buffer);

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
    fn expanded_reasoning_wraps_before_visual_inset_clips_content() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.transcript_mut().clear();
        model.set_window(20, 8);
        model.set_palette(default_palette(), true);
        model.transcript_mut().append_reasoning_message(
            "abcdefghijklmnopqrstuvwxyz",
            ReasoningDisplayMode::Expanded,
            None,
        );

        let buffer = render_model_buffer(&mut model, 20, 8);
        let rows = rendered_rows(&buffer);

        assert!(
            rows.iter().any(|row| row == "  abcdefghijklmnop  "),
            "first reasoning row should fit the inset content width: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row == "  qrstuvwxyz        "),
            "reasoning overflow should wrap to the next row instead of being clipped: {rows:?}"
        );
    }

    #[test]
    fn diff_line_background_fills_the_rendered_row() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.transcript_mut().clear();
        model.set_window(48, 8);
        model.set_palette(default_palette(), true);
        model.append_runtime_tool_activity_from_runtime(RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("one\nold\ntail\n".to_string()),
                new_text: "one\nnew\ntail\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        });

        let buffer = render_model_buffer(&mut model, 48, 8);
        let rows = rendered_rows(&buffer);
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

    fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let _ = model.render_to_buffer(area, &mut buffer);
        buffer
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
