use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    Sender,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, surface_emphasis_style, surface_text_style},
    transcript::{
        DEFAULT_RENDER_WIDTH, render_markdown_lines, wrap_assistant_text, wrap_prompt_text,
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

    /// `render_for_exit` 返回适合退出后打印的消息文本。
    pub fn render_for_exit(
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

    #[cfg(test)]
    fn render_plain_for_test(&self, width: u16) -> String {
        let lines = self.render_lines(width, crate::frontend::tui::theme::default_palette());
        lines_to_plain_text(&lines)
    }
}

fn render_user_message(content: &str, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
    let prefix_width = measure_width(USER_MESSAGE_PREFIX);
    let content_width = usize::from(width.max(1))
        .saturating_sub(prefix_width)
        .max(1);
    let wrapped = wrap_prompt_text(content, content_width, prefix_width);
    format_user_styled_lines(&wrapped, palette)
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

fn format_user_styled_lines(lines: &[String], palette: TerminalPalette) -> Vec<Line<'static>> {
    let prefix_style = surface_text_style(palette);
    let content_style = surface_emphasis_style(palette);
    let continuation_prefix = " ".repeat(measure_width(USER_MESSAGE_PREFIX));

    lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 {
                USER_MESSAGE_PREFIX.to_string()
            } else {
                continuation_prefix.clone()
            };

            Line::default().spans([
                Span::styled(prefix, prefix_style),
                Span::styled(line.clone(), content_style),
            ])
        })
        .collect()
}

fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::theme::default_palette;

    #[test]
    fn assistant_plain_output_preserves_the_raw_command_text() {
        let item = MessageItem::new(Sender::Assistant, "go test ./...");

        assert_eq!(
            item.render_for_exit(6, default_palette(), false),
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

        assert_eq!(lines, vec!["> hello", "  world"]);
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
    fn assistant_exit_render_matches_screen_markdown_text() {
        let item = MessageItem::new(Sender::Assistant, "## Summary\n\n__init__");

        let screen = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(screen, item.render_plain_for_test(20));
    }

    fn plain_line(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
