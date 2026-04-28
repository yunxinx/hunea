use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::TerminalPalette,
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        wrap_prompt_visual_lines,
    },
};

const TOOL_RESULT_PREFIX: &str = "• ";
const TOOL_RESULT_CONTINUATION_PREFIX: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolResultKind {
    Ran,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolResultDisplayLine {
    prefix: &'static str,
    text: String,
}

/// `ToolResultItem` 表示工具审批后的 TUI 展示项，不参与模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResultItem {
    content: String,
    kind: ToolResultKind,
    render_cache_key: u64,
}

impl ToolResultItem {
    /// `new` 创建一条工具审批结果展示项。
    pub(crate) fn new(content: impl Into<String>, kind: ToolResultKind) -> Self {
        let content = content.into();
        let render_cache_key = tool_result_render_cache_key(&content, kind);

        Self {
            content,
            kind,
            render_cache_key,
        }
    }

    /// `render_lines` 将工具审批结果渲染为带颜色的文本行。
    pub(crate) fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let style = self.result_style(palette);

        self.wrapped_lines(width)
            .into_iter()
            .map(|line| {
                Line::from(vec![
                    Span::styled(line.prefix, style),
                    Span::styled(line.text, style),
                ])
            })
            .collect()
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的文本。
    pub(crate) fn render_for_terminal_replay(
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
    pub(crate) fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
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
        let content_char_len = lines
            .iter()
            .map(|line| line.prefix.len() + line.text.len())
            .sum::<usize>();

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

    fn wrapped_lines(&self, width: u16) -> Vec<ToolResultDisplayLine> {
        let width = usize::from(width.max(1));
        self.display_lines()
            .into_iter()
            .enumerate()
            .flat_map(|(logical_line, display_line)| {
                let prefix_width = UnicodeWidthStr::width(display_line.prefix);
                let content_width = width.saturating_sub(prefix_width).max(1);
                wrap_prompt_visual_lines(&display_line.text, content_width, 0)
                    .into_iter()
                    .map(move |line| ToolResultDisplayLine {
                        prefix: display_line.prefix,
                        text: line.text,
                    })
                    .enumerate()
                    .map(move |(wrapped_index, mut line)| {
                        if wrapped_index > 0 || logical_line > 0 {
                            line.prefix = TOOL_RESULT_CONTINUATION_PREFIX;
                        }
                        line
                    })
            })
            .collect()
    }

    fn display_lines(&self) -> Vec<ToolResultDisplayLine> {
        let content_lines = self.content.split('\n').collect::<Vec<_>>();
        if content_lines.is_empty() {
            return vec![ToolResultDisplayLine {
                prefix: TOOL_RESULT_PREFIX,
                text: String::new(),
            }];
        }

        content_lines
            .into_iter()
            .enumerate()
            .map(|(index, content_line)| ToolResultDisplayLine {
                prefix: if index == 0 {
                    TOOL_RESULT_PREFIX
                } else {
                    TOOL_RESULT_CONTINUATION_PREFIX
                },
                text: content_line.to_string(),
            })
            .collect()
    }

    fn result_style(&self, palette: TerminalPalette) -> Style {
        let color = match self.kind {
            ToolResultKind::Ran => palette.quote,
            ToolResultKind::Rejected => palette.system_error,
        };

        if color == Color::Reset {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }
}

fn tool_result_render_cache_key(content: &str, kind: ToolResultKind) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    kind.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;
    use crate::frontend::tui::{
        styled_text::line_to_plain_text,
        theme::{default_palette, terminal_default_palette},
    };

    #[test]
    fn ran_result_uses_quote_color_without_italic() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran cargo test", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "• Ran cargo test");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].style.fg, Some(palette.quote));
        assert!(
            !lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(
            !lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
    }

    #[test]
    fn rejected_result_uses_system_error_color() {
        let palette = default_palette();
        let item = ToolResultItem::new("Reject cargo fmt", ToolResultKind::Rejected);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "• Reject cargo fmt");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.system_error));
        assert_eq!(lines[0].spans[1].style.fg, Some(palette.system_error));
    }

    #[test]
    fn wrapped_result_uses_two_space_continuation_prefix() {
        let item = ToolResultItem::new("Ran very-long-command", ToolResultKind::Ran);
        let lines = item
            .render_lines(10, default_palette())
            .into_iter()
            .map(|line| line_to_plain_text(&line))
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "• Ran".to_string(),
                "  very-lon".to_string(),
                "  g-comman".to_string(),
                "  d".to_string(),
            ]
        );
    }

    #[test]
    fn terminal_default_palette_keeps_reset_style_plain() {
        let item = ToolResultItem::new("Ran echo ok", ToolResultKind::Ran);
        let line = item.render_lines(80, terminal_default_palette()).remove(0);

        assert_eq!(
            line.spans[0].style.fg,
            Some(ratatui::style::Color::LightGreen)
        );
    }
}
