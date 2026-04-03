use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    Sender,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, secondary_text_style, surface_text_style},
    transcript::{
        DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchorKind, render_markdown_lines,
        wrap_assistant_text, wrap_prompt_text, wrap_prompt_visual_lines,
    },
};

const USER_MESSAGE_PREFIX: &str = "> ";

/// `MessageItem` 表示 transcript 中的一条对话消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    sender: Sender,
    content: String,
}

impl MessageItem {
    /// `new` 创建一条消息项。
    pub fn new(sender: Sender, content: impl Into<String>) -> Self {
        Self {
            sender,
            content: content.into(),
        }
    }

    /// `render_lines` 将消息渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self.sender {
            Sender::User => render_user_message(&self.content, width, palette),
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
            Sender::User => render_user_plain_text(&self.content, width),
            Sender::Assistant => {
                lines_to_plain_text(&render_assistant_message(&self.content, width, palette))
            }
        }
    }

    pub(crate) fn render_cache_key(&self) -> String {
        format!("{}:{}", self.sender as u8, self.content)
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        _palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        let prefix_width = user_message_inset_width();
        let content_width = user_message_content_width(width);
        wrap_prompt_visual_lines(&self.content, content_width, prefix_width)
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

    #[cfg(test)]
    fn render_plain_for_test(&self, width: u16) -> String {
        self.render_plain_text(width, crate::frontend::tui::theme::default_palette())
    }
}

fn render_user_message(content: &str, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
    let wrapped = wrap_user_message_lines(content, width);
    format_user_styled_lines(&wrapped, width, palette)
}

fn render_user_plain_text(content: &str, width: u16) -> String {
    format_user_plain_lines(&wrap_user_message_lines(content, width))
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

fn format_user_styled_lines(
    lines: &[String],
    width: u16,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let mut prefix_glyph_style = secondary_text_style(palette);
    if let Some(surface) = palette.surface {
        prefix_glyph_style = prefix_glyph_style.bg(surface);
    }
    let content_style = surface_text_style(palette);
    let continuation_prefix = " ".repeat(user_message_inset_width());
    let total_width = usize::from(width.max(1));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let trailing_fill_width =
                total_width.saturating_sub(user_message_inset_width() + measure_width(line));
            let trailing_fill = " ".repeat(trailing_fill_width);

            if index == 0 {
                Line::default().spans([
                    Span::styled(">", prefix_glyph_style),
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

fn format_user_plain_lines(lines: &[String]) -> String {
    let continuation_prefix = " ".repeat(user_message_inset_width());

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{USER_MESSAGE_PREFIX}{line}")
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

fn wrap_user_message_lines(content: &str, width: u16) -> Vec<String> {
    wrap_prompt_text(
        content,
        user_message_content_width(width),
        user_message_inset_width(),
    )
}

fn user_message_inset_width() -> usize {
    measure_width(USER_MESSAGE_PREFIX)
}

fn user_message_content_width(width: u16) -> usize {
    usize::from(width.max(1))
        .saturating_sub(user_message_inset_width() * 2)
        .max(1)
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
    fn user_render_wraps_prose_at_word_boundaries() {
        let item = MessageItem::new(Sender::User, "hello world");

        let lines = item
            .render_lines(8, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["> hell  ", "  o     ", "  worl  ", "  d     "]);
    }

    #[test]
    fn user_render_uses_secondary_prefix_glyph_and_fills_the_requested_width() {
        let palette = default_palette();
        let item = MessageItem::new(Sender::User, "hello");
        let lines = item.render_lines(20, palette);

        assert_eq!(lines.len(), 1);
        assert_eq!(plain_line(lines[0].clone()), "> hello             ");
        assert_eq!(lines[0].width(), 20);
        assert_eq!(lines[0].spans.len(), 4);
        assert_eq!(
            lines[0].spans[0].style,
            secondary_text_style(palette).bg(palette.surface.unwrap())
        );
        assert_eq!(lines[0].spans[1].style, surface_text_style(palette));
        assert_eq!(lines[0].spans[2].style, surface_text_style(palette));
        assert_eq!(lines[0].spans[3].style, surface_text_style(palette));
    }

    #[test]
    fn user_terminal_replay_keeps_screen_width_padding_in_plain_text() {
        let item = MessageItem::new(Sender::User, "hello");

        assert_eq!(
            item.render_for_terminal_replay(20, default_palette(), false),
            "> hello             "
        );
    }

    #[test]
    fn user_render_preserves_wrapped_continuation_spaces() {
        let item = MessageItem::new(Sender::User, "aaaaaaaaaaaaaaaaaa b   c");

        assert_eq!(
            item.render_plain_for_test(22),
            "> aaaaaaaaaaaaaaaaaa\n  b   c"
        );
    }

    #[test]
    fn user_render_preserves_long_wrapped_leading_spaces() {
        let item = MessageItem::new(Sender::User, "abc d    e");

        assert_eq!(item.render_plain_for_test(7), "> abc\n  d\n     \n   e");
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
