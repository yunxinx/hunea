use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    Sender, StyleMode,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style, surface_emphasis_style, surface_text_style},
    transcript::{
        DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchorKind, render_markdown_lines,
        wrap_assistant_text, wrap_prompt_text, wrap_prompt_visual_lines,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UserMessageRenderLayout {
    frame_width: usize,
    content_width: usize,
    line_prefix_width: usize,
    shows_prefix: bool,
    shows_frame: bool,
}

/// `MessageItem` 表示 transcript 中的一条对话消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    sender: Sender,
    content: String,
    style_mode: StyleMode,
}

impl MessageItem {
    /// `new` 创建一条消息项。
    #[cfg(test)]
    pub fn new(sender: Sender, content: impl Into<String>) -> Self {
        Self::new_with_style_mode(sender, content, StyleMode::Cx)
    }

    /// `new_with_style_mode` 创建一条带指定样式模式的消息项。
    pub fn new_with_style_mode(
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
    ) -> Self {
        Self {
            sender,
            content: content.into(),
            style_mode: style_mode.normalized(),
        }
    }

    /// `render_lines` 将消息渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self.sender {
            Sender::User => {
                render_user_message_lines(&self.content, width, palette, self.style_mode)
            }
            Sender::Assistant => render_assistant_message(&self.content, width, palette),
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的消息文本。
    pub fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let lines = self.render_lines(width, palette);
        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本消息内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        match self.sender {
            Sender::User => render_user_plain_text(&self.content, width, self.style_mode),
            Sender::Assistant => {
                lines_to_plain_text(&render_assistant_message(&self.content, width, palette))
            }
        }
    }

    pub(crate) fn render_cache_key(&self) -> String {
        let style_key = if self.sender == Sender::User {
            match self.style_mode.normalized() {
                StyleMode::Cx => "cx",
                StyleMode::Cc => "cc",
                StyleMode::Ms => "ms",
            }
        } else {
            ""
        };

        format!("{}:{style_key}:{}", self.sender as u8, self.content)
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        match self.style_mode.normalized() {
            StyleMode::Ms => user_message_logical_line_anchors(
                &self.content,
                user_message_legacy_content_width(width, self.style_mode),
                user_message_inset_width(self.style_mode),
            ),
            StyleMode::Cc => user_message_logical_line_anchors(
                &self.content,
                user_message_compact_content_width(width, self.style_mode),
                user_message_inset_width(self.style_mode),
            ),
            StyleMode::Cx => {
                let layout = user_message_layout(width, self.style_mode);
                let wrapped = wrap_prompt_visual_lines(
                    &self.content,
                    layout.content_width,
                    layout.line_prefix_width,
                );
                let has_frame = layout.shows_frame && has_visible_user_message_frame(palette);
                let mut anchors = Vec::with_capacity(wrapped.len() + usize::from(has_frame) * 2);

                if has_frame {
                    anchors.push(rendered_line_anchor(0));
                }

                let rendered_offset = usize::from(has_frame);
                for (index, line) in wrapped.into_iter().enumerate() {
                    anchors.push(ItemLineAnchor {
                        kind: LineAnchorKind::LogicalPosition,
                        logical_line: line.logical_line,
                        range_start: line.visible_start_char,
                        range_end: line.end_char,
                        rendered_line: index + rendered_offset,
                        gap_offset: 0,
                    });
                }

                if has_frame {
                    anchors.push(rendered_line_anchor(anchors.len()));
                }

                anchors
            }
        }
    }

    #[cfg(test)]
    fn render_plain_for_test(&self, width: u16) -> String {
        self.render_plain_text(width, crate::frontend::tui::theme::default_palette())
    }
}

fn render_user_message_lines(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    match style_mode.normalized() {
        StyleMode::Cx => render_framed_user_message(content, width, palette, style_mode),
        StyleMode::Cc => render_compact_user_message(content, width, palette, style_mode),
        StyleMode::Ms => render_legacy_user_message(content, width, palette, style_mode),
    }
}

fn render_framed_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let layout = user_message_layout(width, style_mode);
    let wrapped = wrap_prompt_text(content, layout.content_width, layout.line_prefix_width);
    let mut rendered = format_framed_user_lines(&wrapped, layout, palette, style_mode);
    if !layout.shows_frame || !has_visible_user_message_frame(palette) {
        return rendered;
    }

    let padding_line = user_message_surface_padding_line(layout.frame_width, palette);
    let mut lines = Vec::with_capacity(rendered.len() + 2);
    lines.push(padding_line.clone());
    lines.append(&mut rendered);
    lines.push(padding_line);
    lines
}

fn render_compact_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let wrapped = wrap_prompt_text(
        content,
        user_message_compact_content_width(width, style_mode),
        user_message_inset_width(style_mode),
    );
    format_compact_user_lines(&wrapped, usize::from(width.max(1)), palette, style_mode)
}

fn render_legacy_user_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let wrapped = wrap_prompt_text(
        content,
        user_message_legacy_content_width(width, style_mode),
        user_message_inset_width(style_mode),
    );
    format_legacy_user_lines(&wrapped, palette, style_mode)
}

fn render_assistant_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let width = if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    };
    let rendered = render_markdown_lines(content, width, palette);
    if rendered.is_empty() {
        return wrap_assistant_text(content, width, 0)
            .into_iter()
            .map(Line::raw)
            .collect();
    }

    rendered
}

fn render_user_plain_text(content: &str, width: u16, style_mode: StyleMode) -> String {
    match style_mode.normalized() {
        StyleMode::Cx | StyleMode::Cc => {
            let wrapped = wrap_prompt_text(
                content,
                user_message_compact_content_width(width, style_mode),
                user_message_inset_width(style_mode),
            );
            format_user_plain_lines(&wrapped, style_mode)
        }
        StyleMode::Ms => {
            let wrapped = wrap_prompt_text(
                content,
                user_message_legacy_content_width(width, style_mode),
                user_message_inset_width(style_mode),
            );
            format_user_plain_lines(&wrapped, style_mode)
        }
    }
}

fn format_framed_user_lines(
    lines: &[String],
    layout: UserMessageRenderLayout,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(layout.line_prefix_width);

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trailing_fill_width = layout
                .frame_width
                .saturating_sub(layout.line_prefix_width + measure_width(line));
            let trailing_fill = " ".repeat(trailing_fill_width);

            if index == 0 && layout.shows_prefix {
                Line::default().spans([
                    Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
                    Span::styled(" ", prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            }
        })
        .collect()
}

fn format_compact_user_lines(
    lines: &[String],
    width: usize,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trailing_fill_width =
                width.saturating_sub(user_message_inset_width(style_mode) + measure_width(line));
            let trailing_fill = " ".repeat(trailing_fill_width);

            if index == 0 {
                Line::default().spans([
                    Span::styled(user_message_prefix_glyph(style_mode), prefix_glyph_style),
                    Span::styled(" ", prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                    Span::styled(trailing_fill, prefix_style),
                ])
            }
        })
        .collect()
}

fn format_legacy_user_lines(
    lines: &[String],
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let content_style = surface_emphasis_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                Line::default().spans([
                    Span::styled(user_message_prefix(style_mode), prefix_style),
                    Span::styled(line.clone(), content_style),
                ])
            } else {
                Line::default().spans([
                    Span::styled(continuation_prefix.clone(), prefix_style),
                    Span::styled(line.clone(), content_style),
                ])
            }
        })
        .collect()
}

fn format_user_plain_lines(lines: &[String], style_mode: StyleMode) -> String {
    let continuation_prefix = " ".repeat(user_message_inset_width(style_mode));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{}{}", user_message_prefix(style_mode), line)
            } else {
                format!("{continuation_prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn rendered_line_anchor(rendered_line: usize) -> ItemLineAnchor {
    ItemLineAnchor {
        kind: LineAnchorKind::RenderedLine,
        logical_line: 0,
        range_start: 0,
        range_end: 0,
        rendered_line,
        gap_offset: 0,
    }
}

fn user_message_logical_line_anchors(
    content: &str,
    content_width: usize,
    line_prefix_width: usize,
) -> Vec<ItemLineAnchor> {
    wrap_prompt_visual_lines(content, content_width, line_prefix_width)
        .into_iter()
        .enumerate()
        .map(|(rendered_line, line)| ItemLineAnchor {
            kind: LineAnchorKind::LogicalPosition,
            logical_line: line.logical_line,
            range_start: line.visible_start_char,
            range_end: line.end_char,
            rendered_line,
            gap_offset: 0,
        })
        .collect()
}

fn has_visible_user_message_frame(palette: TerminalPalette) -> bool {
    palette.surface.is_some()
}

fn user_message_surface_padding_line(width: usize, palette: TerminalPalette) -> Line<'static> {
    Line::default().spans([Span::styled(
        " ".repeat(width.max(1)),
        surface_text_style(palette),
    )])
}

fn user_message_prefix(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "› ",
        StyleMode::Cc => "❯ ",
        StyleMode::Ms => "> ",
    }
}

fn user_message_prefix_glyph(style_mode: StyleMode) -> &'static str {
    match style_mode.normalized() {
        StyleMode::Cx => "›",
        StyleMode::Cc => "❯",
        StyleMode::Ms => ">",
    }
}

fn user_message_inset_width(style_mode: StyleMode) -> usize {
    measure_width(user_message_prefix(style_mode))
}

fn user_message_compact_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode) * 2)
        .max(1)
}

fn user_message_legacy_content_width(width: u16, style_mode: StyleMode) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width(style_mode))
        .max(1)
}

fn user_message_layout(width: u16, style_mode: StyleMode) -> UserMessageRenderLayout {
    let content_width = user_message_compact_content_width(width, style_mode);
    UserMessageRenderLayout {
        frame_width: usize::from(width.max(1))
            .max(user_message_inset_width(style_mode) + content_width),
        content_width,
        line_prefix_width: user_message_inset_width(style_mode),
        shows_prefix: true,
        shows_frame: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::theme::{default_palette, secondary_text_style, surface_text_style};

    #[test]
    fn assistant_plain_output_preserves_the_raw_command_text() {
        let item = MessageItem::new(Sender::Assistant, "go test ./...");

        assert_eq!(
            item.render_plain_text(6, default_palette()),
            "go\ntest\n./..."
        );
    }

    #[test]
    fn legacy_user_render_wraps_prose_at_word_boundaries() {
        let item = MessageItem::new_with_style_mode(Sender::User, "hello world", StyleMode::Ms);

        let lines = item
            .render_lines(8, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["> hello", "  world"]);
    }

    #[test]
    fn cx_user_render_adds_surface_padding_lines() {
        let palette = default_palette();
        let item = MessageItem::new(Sender::User, "hello");
        let lines = item.render_lines(20, palette);

        assert_eq!(lines.len(), 3);
        assert_eq!(plain_line(lines[0].clone()), "                    ");
        assert_eq!(plain_line(lines[1].clone()), "› hello             ");
        assert_eq!(plain_line(lines[2].clone()), "                    ");
        assert_eq!(lines[1].width(), 20);
        assert_eq!(lines[1].spans.len(), 4);
        assert_eq!(
            lines[1].spans[0].style,
            secondary_text_style(palette).bg(palette.surface.unwrap())
        );
        assert_eq!(lines[1].spans[1].style, surface_text_style(palette));
        assert_eq!(lines[1].spans[2].style, surface_text_style(palette));
        assert_eq!(lines[1].spans[3].style, surface_text_style(palette));
    }

    #[test]
    fn cc_user_terminal_replay_keeps_compact_prefix() {
        let item = MessageItem::new_with_style_mode(Sender::User, "hello", StyleMode::Cc);

        assert_eq!(
            item.render_for_terminal_replay(20, default_palette(), false),
            "❯ hello             "
        );
    }

    #[test]
    fn legacy_user_render_preserves_wrapped_continuation_spaces() {
        let item = MessageItem::new_with_style_mode(
            Sender::User,
            "aaaaaaaaaaaaaaaaaa b   c",
            StyleMode::Ms,
        );

        assert_eq!(
            item.render_plain_for_test(22),
            "> aaaaaaaaaaaaaaaaaa\n  b   c"
        );
    }

    #[test]
    fn legacy_user_render_preserves_long_wrapped_leading_spaces() {
        let item = MessageItem::new_with_style_mode(Sender::User, "abc d    e", StyleMode::Ms);

        assert_eq!(item.render_plain_for_test(7), "> abc d\n      e");
    }

    #[test]
    fn assistant_render_wraps_leading_make_explanation_as_prose() {
        let item = MessageItem::new(Sender::Assistant, "make the handler return early");

        let lines = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["make the handler", "return early"]);
    }

    #[test]
    fn assistant_render_uses_markdown_heading_rendering() {
        let item = MessageItem::new(Sender::Assistant, "# Overview of the API");

        let lines = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["Overview of the API"]);
    }

    #[test]
    fn assistant_render_uses_markdown_emphasis_rendering() {
        let item = MessageItem::new(Sender::Assistant, "__init__");

        let lines = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["init"]);
    }

    #[test]
    fn assistant_terminal_replay_matches_screen_markdown_text() {
        let item = MessageItem::new(Sender::Assistant, "## Summary\n\n__init__");

        let screen = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(
            screen,
            item.render_for_terminal_replay(20, default_palette(), false)
        );
    }

    fn plain_line(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
