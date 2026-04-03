use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::{
    Sender,
    theme::{TerminalPalette, surface_emphasis_style, surface_text_style},
    transcript::{DEFAULT_RENDER_WIDTH, wrap_assistant_text, wrap_prompt_text},
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

    /// `render_plain` 返回用于退出后打印的纯文本内容。
    pub fn render_plain(&self, width: u16) -> String {
        match self.sender {
            Sender::User => render_user_plain(&self.content, width),
            Sender::Assistant => render_assistant_plain(&self.content, width),
        }
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
    _palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let width = if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    };
    let (left_padding, right_padding) = assistant_message_padding(content, width);
    let content_width = width.saturating_sub(left_padding + right_padding).max(1);

    wrap_assistant_text(content, content_width, left_padding)
        .into_iter()
        .map(|line| {
            Line::default().spans([
                Span::styled(" ".repeat(left_padding), Style::new()),
                Span::styled(line, Style::new()),
                Span::styled(" ".repeat(right_padding), Style::new()),
            ])
        })
        .collect()
}

fn render_user_plain(content: &str, width: u16) -> String {
    let _ = width;
    let raw_lines = content
        .split('\n')
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    format_user_plain_lines(&raw_lines)
}

fn render_assistant_plain(content: &str, width: u16) -> String {
    let _ = width;
    content.to_string()
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

fn format_user_plain_lines(lines: &[String]) -> String {
    let continuation_prefix = " ".repeat(measure_width(USER_MESSAGE_PREFIX));

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

fn assistant_message_padding(content: &str, width: usize) -> (usize, usize) {
    if width <= 1 {
        return (0, 0);
    }

    let mut min_content_width = widest_non_tab_cluster_width(content).max(1);
    if min_content_width > width {
        min_content_width = width;
    }

    let total_padding = width.saturating_sub(min_content_width).min(4);
    let left = 2.min(total_padding.div_ceil(2));
    let right = 2.min(total_padding.saturating_sub(left));

    (left, right)
}

fn widest_non_tab_cluster_width(content: &str) -> usize {
    UnicodeSegmentation::graphemes(content, true)
        .filter(|cluster| *cluster != "\t")
        .map(measure_width)
        .max()
        .unwrap_or(0)
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

        assert_eq!(item.render_plain(6), "go test ./...");
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
    fn assistant_render_wraps_leading_make_explanation_as_prose() {
        let item = MessageItem::new(Sender::Assistant, "make the handler return early");

        let lines = item
            .render_lines(20, default_palette())
            .into_iter()
            .map(plain_line)
            .collect::<Vec<_>>();

        assert_eq!(lines, vec!["  make the handler  ", "  return early  "]);
    }

    fn plain_line(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
