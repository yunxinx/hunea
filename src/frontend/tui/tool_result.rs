use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::transcript::markdown_highlight::HighlightChunk;
use super::{
    styled_text::{line_to_plain_text, lines_to_ansi_text, lines_to_plain_text},
    theme::TerminalPalette,
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        markdown_highlight::{highlight_code_chunks, wrap_highlight_chunks},
        wrap_prompt_visual_lines,
    },
};

const TOOL_RESULT_PREFIX: &str = "● ";
const TOOL_RESULT_CONTINUATION_PREFIX: &str = "  ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ToolResultKind {
    Ran,
    Rejected,
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
        self.wrapped_styled_lines(width, palette)
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
        palette: TerminalPalette,
    ) -> (usize, usize) {
        let lines = self.wrapped_styled_lines(width, palette);
        let content_char_len = lines
            .iter()
            .map(|line| line_to_plain_text(line).len())
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

    fn wrapped_styled_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        self.content
            .split('\n')
            .enumerate()
            .flat_map(|(logical_line, content_line)| {
                self.wrap_logical_line(content_line, logical_line, width, palette)
            })
            .collect()
    }

    fn wrap_logical_line(
        &self,
        content_line: &str,
        logical_line: usize,
        width: usize,
        palette: TerminalPalette,
    ) -> Vec<Line<'static>> {
        let initial_prefix = if logical_line == 0 {
            TOOL_RESULT_PREFIX
        } else {
            TOOL_RESULT_CONTINUATION_PREFIX
        };
        let prefix_width = UnicodeWidthStr::width(initial_prefix);
        let content_width = width.saturating_sub(prefix_width).max(1);
        let logical_lines = self.wrap_content_line(content_line, content_width, palette);

        if logical_lines.is_empty() {
            return vec![Line::from(vec![self.prefix_span(initial_prefix, palette)])];
        }

        logical_lines
            .into_iter()
            .enumerate()
            .map(|(wrapped_index, content_spans)| {
                let prefix = if wrapped_index == 0 {
                    initial_prefix
                } else {
                    TOOL_RESULT_CONTINUATION_PREFIX
                };
                let mut spans = Vec::with_capacity(content_spans.len() + 1);
                spans.push(self.prefix_span(prefix, palette));
                spans.extend(content_spans);
                Line::from(spans)
            })
            .collect()
    }

    fn wrap_content_line(
        &self,
        content_line: &str,
        width: usize,
        _palette: TerminalPalette,
    ) -> Vec<Vec<Span<'static>>> {
        let Some(parsed) = ParsedToolResultLine::parse(content_line) else {
            return self.wrap_plain_content(content_line, width);
        };

        if !parsed.should_highlight_as_shell {
            return self.wrap_plain_result_content(&parsed.non_shell_display_text(), width);
        }

        self.wrap_shell_result_content(parsed, width)
    }

    fn wrap_plain_content(&self, content_line: &str, width: usize) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| vec![Span::raw(line.text)])
            .collect()
    }

    fn wrap_plain_result_content(
        &self,
        content_line: &str,
        width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        wrap_prompt_visual_lines(content_line, width, 0)
            .into_iter()
            .map(|line| style_core_result_line(line.text))
            .collect()
    }

    fn wrap_shell_result_content(
        &self,
        parsed: ParsedToolResultLine<'_>,
        width: usize,
    ) -> Vec<Vec<Span<'static>>> {
        let mut chunks = vec![HighlightChunk {
            text: parsed.verb.to_string(),
            style: Style::new().add_modifier(Modifier::BOLD),
        }];

        if !parsed.body.is_empty() {
            chunks.push(HighlightChunk {
                text: " ".to_string(),
                style: Style::new(),
            });
            chunks.extend(self.shell_command_chunks(parsed.body));
        }

        wrap_highlight_chunks(&[chunks], width)
    }

    fn shell_command_chunks(&self, command: &str) -> Vec<HighlightChunk> {
        highlight_code_chunks(command, "bash", Style::new())
            .map(|highlighted| highlighted.into_iter().flatten().collect::<Vec<_>>())
            .filter(|chunks| !chunks.is_empty())
            .unwrap_or_else(|| {
                vec![HighlightChunk {
                    text: command.to_string(),
                    style: Style::new(),
                }]
            })
    }

    fn prefix_span(&self, prefix: &'static str, palette: TerminalPalette) -> Span<'static> {
        Span::styled(prefix, self.result_style(palette))
    }
    fn result_style(&self, palette: TerminalPalette) -> Style {
        let color = match self.kind {
            ToolResultKind::Ran => palette.quote,
            ToolResultKind::Rejected => palette.approval_rejected,
        };

        if color == Color::Reset {
            Style::new()
        } else {
            Style::new().fg(color)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedToolResultLine<'a> {
    verb: &'a str,
    body: &'a str,
    should_highlight_as_shell: bool,
}

impl<'a> ParsedToolResultLine<'a> {
    fn parse(content_line: &'a str) -> Option<Self> {
        let (verb, body) = split_verb(content_line)?;
        let body = body.trim_start();
        let (body, has_shell_prefix) = body
            .strip_prefix("Shell:")
            .map(|command| (command.trim_start(), true))
            .unwrap_or((body, false));
        let should_highlight_as_shell = has_shell_prefix || looks_like_shell_command(body);

        Some(Self {
            verb,
            body,
            should_highlight_as_shell,
        })
    }

    fn non_shell_display_text(self) -> String {
        match self.verb {
            "Ran" => self.body.to_string(),
            "Reject" => {
                let rejected_body = strip_redundant_reject_title_verb(self.body);
                if rejected_body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, rejected_body)
                }
            }
            _ => {
                if self.body.is_empty() {
                    self.verb.to_string()
                } else {
                    format!("{} {}", self.verb, self.body)
                }
            }
        }
    }
}

fn split_verb(content_line: &str) -> Option<(&str, &str)> {
    for verb in ["Ran", "Reject"] {
        if content_line == verb {
            return Some((verb, ""));
        }
        if let Some(body) = content_line.strip_prefix(verb)
            && body.starts_with(char::is_whitespace)
        {
            return Some((verb, body));
        }
    }

    None
}

fn looks_like_shell_command(body: &str) -> bool {
    let Some(first) = body.trim_start().chars().next() else {
        return false;
    };

    first.is_ascii_lowercase()
        || first.is_ascii_digit()
        || matches!(first, '.' | '/' | '~' | '$' | '\'' | '"' | '`')
}

fn strip_redundant_reject_title_verb(text: &str) -> &str {
    let text = text.trim_start();
    text.strip_prefix("Run ")
        .map(str::trim_start)
        .unwrap_or(text)
}

fn style_core_result_line(line: String) -> Vec<Span<'static>> {
    let Some((core, rest)) = split_first_word(&line) else {
        return vec![Span::raw(line)];
    };

    if rest.is_empty() {
        return vec![Span::styled(
            core.to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        )];
    }

    vec![
        Span::styled(core.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::raw(rest.to_string()),
    ]
}

fn split_first_word(line: &str) -> Option<(&str, &str)> {
    if line.is_empty() {
        return None;
    }

    let Some((index, _)) = line.char_indices().find(|(_, ch)| ch.is_whitespace()) else {
        return Some((line, ""));
    };

    Some((&line[..index], &line[index..]))
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
        let item = ToolResultItem::new("Ran Write file", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● Write file");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Write");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[2].style.fg.is_none());
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
    fn rejected_result_uses_approval_rejected_color() {
        let palette = default_palette();
        let item = ToolResultItem::new("Reject Run destructive command", ToolResultKind::Rejected);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            line_to_plain_text(&lines[0]),
            "● Reject destructive command"
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.approval_rejected));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Reject");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(lines[0].spans[2].style.fg.is_none());
    }

    #[test]
    fn rejected_non_shell_result_preserves_non_run_title_action() {
        let item = ToolResultItem::new("Reject Write file", ToolResultKind::Rejected);
        let lines = item.render_lines(80, default_palette());

        assert_eq!(line_to_plain_text(&lines[0]), "● Reject Write file");
    }

    #[test]
    fn shell_result_removes_shell_prefix_and_highlights_command() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran Shell: cat Cargo.toml", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(line_to_plain_text(&lines[0]), "● Ran cat Cargo.toml");
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(2)
                .any(|span| span.style.fg.is_some()),
            "shell command spans should carry syntax highlight foreground colors: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn naked_shell_result_highlights_command() {
        let palette = default_palette();
        let item = ToolResultItem::new("Ran sed -n '1,80p' src/main.rs", ToolResultKind::Ran);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            line_to_plain_text(&lines[0]),
            "● Ran sed -n '1,80p' src/main.rs"
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
        assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
        assert!(lines[0].spans[1].style.fg.is_none());
        assert!(
            lines[0]
                .spans
                .iter()
                .skip(2)
                .any(|span| span.style.fg.is_some()),
            "naked shell command spans should carry syntax highlight foreground colors: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn wrapped_shell_result_uses_continuation_prefix_and_keeps_highlight() {
        let item = ToolResultItem::new(
            "Ran sed -n '1,80p' src/frontend/tui/tool_result.rs",
            ToolResultKind::Ran,
        );
        let lines = item.render_lines(18, default_palette());
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert!(
            plain_lines.len() > 1,
            "shell command should wrap in a narrow viewport: {plain_lines:?}"
        );
        assert!(
            plain_lines[0].starts_with("● Ran "),
            "first shell line should keep the status prefix and verb: {plain_lines:?}"
        );
        assert!(
            plain_lines[1..].iter().all(|line| line.starts_with("  ")),
            "wrapped shell continuation lines should use two leading spaces: {plain_lines:?}"
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter().skip(1))
                .any(|span| span.style.fg.is_some()),
            "wrapped shell command spans should keep syntax highlight foreground colors: {lines:?}"
        );
    }

    #[test]
    fn wrapped_result_uses_two_space_continuation_prefix() {
        let item = ToolResultItem::new("Ran Very-long-command", ToolResultKind::Ran);
        let lines = item
            .render_lines(10, default_palette())
            .into_iter()
            .map(|line| line_to_plain_text(&line))
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "● Very-lon".to_string(),
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
