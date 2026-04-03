use ratatui::{
    style::Style,
    text::{Line, Span},
};

use super::{
    Sender,
    theme::{TerminalPalette, surface_emphasis_style, surface_text_style},
};

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
    let prefix = "> ";
    let continuation_prefix = " ".repeat(prefix.chars().count());
    let content_width = width.max(1) as usize - prefix.chars().count().min(width.max(1) as usize);
    let content_width = content_width.max(1);
    let wrapped = wrap_text(content, content_width);
    let prefix_style = surface_text_style(palette);
    let content_style = surface_emphasis_style(palette);

    wrapped
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let line_prefix = if index == 0 {
                prefix
            } else {
                continuation_prefix.as_str()
            };

            Line::default().spans([
                Span::styled(line_prefix.to_string(), prefix_style),
                Span::styled(line, content_style),
            ])
        })
        .collect()
}

fn render_assistant_message(
    content: &str,
    width: u16,
    _palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let padded_width = width.saturating_sub(2).max(1) as usize;

    wrap_text(content, padded_width)
        .into_iter()
        .map(|line| {
            Line::default().spans([
                Span::styled("  ".to_string(), Style::new()),
                Span::styled(line, Style::new()),
            ])
        })
        .collect()
}

fn render_user_plain(content: &str, width: u16) -> String {
    let prefix = "> ";
    let continuation_prefix = " ".repeat(prefix.chars().count());
    let content_width = width.max(1) as usize - prefix.chars().count().min(width.max(1) as usize);
    let content_width = content_width.max(1);

    wrap_text(content, content_width)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                format!("{prefix}{line}")
            } else {
                format!("{continuation_prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_assistant_plain(content: &str, width: u16) -> String {
    let padded_width = width.saturating_sub(2).max(1) as usize;
    wrap_text(content, padded_width)
        .into_iter()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn wrap_text(content: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for raw_line in content.split('\n') {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut current_width = 0usize;

        for character in raw_line.chars() {
            if current_width == width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }

            current.push(character);
            current_width += 1;
        }

        if current.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}
