use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::{
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, system_error_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        wrap_prompt_visual_lines,
    },
};

const SYSTEM_MESSAGE_PREFIX: &str = "■ ";
const SYSTEM_MESSAGE_CONTINUATION_PREFIX: &str = "  ";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemMessageDisplayLine {
    prefix: &'static str,
    text: String,
}

/// `SystemMessageItem` 表示只属于 TUI 的运行时提示，不参与模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemMessageItem {
    content: String,
    render_cache_key: u64,
}

impl SystemMessageItem {
    /// `new` 创建一条运行时 system message。
    pub fn new(content: impl Into<String>) -> Self {
        let content = content.into();
        let render_cache_key = system_message_render_cache_key(&content);

        Self {
            content,
            render_cache_key,
        }
    }

    /// `render_lines` 将 system message 渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let style = system_error_text_style(palette);

        self.wrapped_lines(width)
            .into_iter()
            .map(|line| Line::styled(line.text, style))
            .collect()
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
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

    /// `render_plain_text` 返回不带 ANSI 的纯文本内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        lines_to_plain_text(&self.render_lines(width, palette))
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        self.content.len()
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        _palette: TerminalPalette,
    ) -> (usize, usize) {
        let lines = self.wrapped_lines(width);
        let content_char_len = lines.iter().map(|line| line.text.len()).sum::<usize>();

        (lines.len(), content_char_len)
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let previous_metrics =
            previous_metrics.filter(|metrics| metrics.cache_key == self.render_cache_key);
        if let Some(metrics) = previous_metrics
            && metrics.is_valid
            && metrics.width == width
        {
            return TranscriptFastEstimate {
                content_line_count: metrics.content_line_count,
                content_char_len: metrics.content_char_len,
                kind: TranscriptEstimateKind::NonAssistant,
                ..TranscriptFastEstimate::default()
            };
        }

        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::NonAssistant,
            ..TranscriptFastEstimate::default()
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        _width: u16,
        _palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        Vec::new()
    }

    fn wrapped_lines(&self, width: u16) -> Vec<super::transcript::PromptVisualLine> {
        let width = usize::from(width.max(1));
        self.display_lines()
            .into_iter()
            .enumerate()
            .flat_map(|(logical_line, display_line)| {
                let prefix_width = UnicodeWidthStr::width(display_line.prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                wrap_prompt_visual_lines(&display_line.text, content_width, 0)
                    .into_iter()
                    .map(move |mut line| {
                        line.text = format!("{}{}", display_line.prefix, line.text);
                        line.logical_line = logical_line;
                        line
                    })
            })
            .collect()
    }

    fn display_lines(&self) -> Vec<SystemMessageDisplayLine> {
        let content_lines = self.content.split('\n').collect::<Vec<_>>();
        if content_lines.is_empty() {
            return vec![SystemMessageDisplayLine {
                prefix: SYSTEM_MESSAGE_PREFIX,
                text: String::new(),
            }];
        }

        let mut lines = Vec::with_capacity(content_lines.len());
        for (index, content_line) in content_lines.iter().enumerate() {
            let prefix = if index == 0 {
                SYSTEM_MESSAGE_PREFIX
            } else {
                SYSTEM_MESSAGE_CONTINUATION_PREFIX
            };

            if index + 1 == content_lines.len()
                && let Some(json_lines) = formatted_json_body_lines(content_line)
            {
                lines.extend(
                    json_lines
                        .into_iter()
                        .map(|line| SystemMessageDisplayLine { prefix, text: line }),
                );
                continue;
            }

            lines.push(SystemMessageDisplayLine {
                prefix,
                text: (*content_line).to_string(),
            });
        }

        lines
    }
}

fn formatted_json_body_lines(line: &str) -> Option<Vec<String>> {
    let body = line.trim_start().strip_prefix("Body:")?.trim();
    if body.is_empty() {
        return None;
    }

    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let formatted = serde_json::to_string_pretty(&value).ok()?;
    Some(formatted.lines().map(str::to_string).collect())
}

fn system_message_render_cache_key(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    "system_message".hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        styled_text::line_to_plain_text,
        theme::{default_palette, system_error_text_style},
    };

    #[test]
    fn system_message_renders_prefix_and_error_color() {
        let palette = default_palette();
        let item = SystemMessageItem::new("connection refused");
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["■ connection refused".to_string()]
        );
        assert_eq!(lines[0].style, system_error_text_style(palette));
    }

    #[test]
    fn system_message_indents_multiline_details_under_prefix() {
        let palette = default_palette();
        let item =
            SystemMessageItem::new("HTTP error.\nCause: bad request\nStatus: 400 Bad Request");
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec![
                "■ HTTP error.".to_string(),
                "  Cause: bad request".to_string(),
                "  Status: 400 Bad Request".to_string(),
            ]
        );
    }

    #[test]
    fn system_message_formats_json_body_as_indented_block() {
        let palette = default_palette();
        let item = SystemMessageItem::new(
            "Invalid status code 400 Bad Request with message:\nBody: {\"error\":{\"code\":\"400\",\"message\":\"Param Incorrect\"}}",
        );
        let lines = item.render_lines(120, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec![
                "■ Invalid status code 400 Bad Request with message:".to_string(),
                "  {".to_string(),
                "    \"error\": {".to_string(),
                "      \"code\": \"400\",".to_string(),
                "      \"message\": \"Param Incorrect\"".to_string(),
                "    }".to_string(),
                "  }".to_string(),
            ]
        );
    }
}
