use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, tertiary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        wrap_assistant_text,
    },
};

const REASONING_ACTION_SHOW: &str = "Show reasoning";
const REASONING_ACTION_HIDE: &str = "Hide reasoning";

/// `ReasoningDisplayMode` 表示思维链消息进入 transcript 时的默认展示状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ReasoningDisplayMode {
    #[default]
    Collapsed,
    Expanded,
}

/// `ReasoningMessageItem` 表示只用于展示的模型思维链，不参与后续模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningMessageItem {
    content: String,
    is_collapsed: bool,
    duration: Option<Duration>,
    render_cache_key: u64,
}

impl ReasoningMessageItem {
    /// `new` 创建一条思维链展示项。
    pub fn new(
        content: impl Into<String>,
        display_mode: ReasoningDisplayMode,
        duration: Option<Duration>,
    ) -> Self {
        let content = content.into();
        let is_collapsed = matches!(display_mode, ReasoningDisplayMode::Collapsed);
        let render_cache_key = reasoning_message_render_cache_key(&content, is_collapsed, duration);

        Self {
            content,
            is_collapsed,
            duration,
            render_cache_key,
        }
    }

    /// `render_lines` 将思维链渲染为淡色斜体文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let style = tertiary_text_style(palette).italic();
        let mut lines = vec![Line::from(Span::styled(self.header_label(), style))];
        if self.is_collapsed {
            return lines;
        }

        lines.extend(
            self.wrapped_lines(width)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, style))),
        );
        lines
    }

    pub(crate) fn is_header_line(&self, line_index: usize) -> bool {
        line_index == 0
    }

    pub(crate) fn header_display_width(&self) -> usize {
        self.header_label().width()
    }

    pub(crate) fn toggle(&mut self) {
        self.is_collapsed = !self.is_collapsed;
        self.render_cache_key =
            reasoning_message_render_cache_key(&self.content, self.is_collapsed, self.duration);
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
        let lines = self.plain_lines(width);
        let content_char_len = lines.iter().map(String::len).sum::<usize>();

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
                kind: TranscriptEstimateKind::Assistant,
                ..TranscriptFastEstimate::default()
            };
        }

        let (content_line_count, content_char_len) = self.measure_render_metrics(width, palette);
        TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::Assistant,
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

    fn wrapped_lines(&self, width: u16) -> Vec<String> {
        wrap_assistant_text(&self.content, usize::from(width.max(1)), 0)
    }

    fn plain_lines(&self, width: u16) -> Vec<String> {
        let mut lines = vec![self.header_label()];
        if !self.is_collapsed {
            lines.extend(self.wrapped_lines(width));
        }
        lines
    }

    fn header_label(&self) -> String {
        let action = if self.is_collapsed {
            REASONING_ACTION_SHOW
        } else {
            REASONING_ACTION_HIDE
        };
        let Some(duration) = self.duration.map(format_reasoning_duration) else {
            return format!("[{action}]");
        };
        format!("[{action} · thoughts {duration}]")
    }
}

fn reasoning_message_render_cache_key(
    content: &str,
    is_collapsed: bool,
    duration: Option<Duration>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "reasoning_message".hash(&mut hasher);
    is_collapsed.hash(&mut hasher);
    duration.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}

fn format_reasoning_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds == 0 && duration > Duration::ZERO {
        return "<1s".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s");
    }
    if seconds < 3600 {
        let minutes = seconds / 60;
        let seconds = seconds % 60;
        return format!("{minutes}m{seconds:02}s");
    }

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    format!("{hours}h{minutes:02}m{seconds:02}s")
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;
    use crate::frontend::tui::{
        styled_text::line_to_plain_text,
        theme::{default_palette, tertiary_text_style},
    };

    #[test]
    fn reasoning_message_renders_dim_italic_text() {
        let palette = default_palette();
        let item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Expanded, None);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["[Hide reasoning]".to_string(), "先分析".to_string()]
        );
        assert_eq!(lines[0].spans[0].style.fg, tertiary_text_style(palette).fg);
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
    }

    #[test]
    fn reasoning_message_collapses_to_clickable_header() {
        let item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Collapsed, None);
        let lines = item.render_lines(80, default_palette());

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["[Show reasoning]".to_string()]
        );
        assert!(item.is_header_line(0));
        assert!(!item.is_header_line(1));
    }

    #[test]
    fn reasoning_message_toggle_changes_rendered_lines_and_cache_key() {
        let mut item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Collapsed, None);
        let collapsed_key = item.render_cache_key();

        item.toggle();

        assert_ne!(item.render_cache_key(), collapsed_key);
        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["[Hide reasoning]".to_string(), "先分析".to_string()]
        );
    }

    #[test]
    fn reasoning_message_header_includes_duration_when_available() {
        let item = ReasoningMessageItem::new(
            "先分析",
            ReasoningDisplayMode::Collapsed,
            Some(Duration::from_secs(3)),
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["[Show reasoning · thoughts 3s]".to_string()]
        );
    }

    #[test]
    fn reasoning_message_header_uses_subsecond_duration_label() {
        let item = ReasoningMessageItem::new(
            "先分析",
            ReasoningDisplayMode::Expanded,
            Some(Duration::from_millis(120)),
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec![
                "[Hide reasoning · thoughts <1s]".to_string(),
                "先分析".to_string()
            ]
        );
    }
}
