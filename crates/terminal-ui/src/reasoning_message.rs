use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::{
    message::assistant_message_content_width,
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
    Snippet,
}

/// `ReasoningMessageItem` 表示只用于展示的模型思维链，不参与后续模型上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningMessageItem {
    content: String,
    display_mode: ReasoningDisplayMode,
    has_toggle_header: bool,
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
        let content = if matches!(display_mode, ReasoningDisplayMode::Snippet) {
            String::new()
        } else {
            content.into()
        };
        let has_toggle_header = matches!(display_mode, ReasoningDisplayMode::Collapsed);
        let render_cache_key =
            reasoning_message_render_cache_key(&content, display_mode, has_toggle_header, duration);

        Self {
            content,
            display_mode,
            has_toggle_header,
            duration,
            render_cache_key,
        }
    }

    /// `render_lines` 将思维链渲染为淡色斜体文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        let style = tertiary_text_style(palette).italic();

        self.plain_lines(width)
            .into_iter()
            .map(|line| Line::from(Span::styled(line, style)))
            .collect()
    }

    pub(crate) fn is_header_line(&self, line_index: usize) -> bool {
        line_index == 0 && self.is_toggleable()
    }

    pub(crate) fn uses_assistant_visual_inset(&self) -> bool {
        !matches!(self.display_mode, ReasoningDisplayMode::Snippet)
    }

    pub(crate) fn header_display_width(&self) -> usize {
        if self.is_toggleable() {
            self.header_label().width()
        } else {
            0
        }
    }

    pub(crate) fn toggle(&mut self) {
        if !self.is_toggleable() {
            return;
        }

        self.display_mode = match self.display_mode {
            ReasoningDisplayMode::Collapsed => ReasoningDisplayMode::Expanded,
            ReasoningDisplayMode::Expanded => ReasoningDisplayMode::Collapsed,
            ReasoningDisplayMode::Snippet => return,
        };
        self.render_cache_key = reasoning_message_render_cache_key(
            &self.content,
            self.display_mode,
            self.has_toggle_header,
            self.duration,
        );
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

    fn plain_lines(&self, width: u16) -> Vec<String> {
        match self.display_mode {
            ReasoningDisplayMode::Collapsed | ReasoningDisplayMode::Snippet => {
                vec![self.header_label()]
            }
            ReasoningDisplayMode::Expanded if self.has_toggle_header => {
                let mut lines = vec![self.header_label()];
                lines.extend(self.wrapped_lines(width));
                lines
            }
            ReasoningDisplayMode::Expanded => self.wrapped_lines(width),
        }
    }

    fn wrapped_lines(&self, width: u16) -> Vec<String> {
        wrap_assistant_text(&self.content, assistant_message_content_width(width), 0)
    }

    fn header_label(&self) -> String {
        match self.display_mode {
            ReasoningDisplayMode::Collapsed | ReasoningDisplayMode::Expanded => {
                let action = if matches!(self.display_mode, ReasoningDisplayMode::Collapsed) {
                    REASONING_ACTION_SHOW
                } else {
                    REASONING_ACTION_HIDE
                };
                let Some(duration) = self.duration.map(format_reasoning_duration) else {
                    return format!("[{action}]");
                };
                format!("[{action} · thoughts {duration}]")
            }
            ReasoningDisplayMode::Snippet => {
                let Some(duration) = self.duration.map(format_reasoning_duration) else {
                    return "• thoughts".to_string();
                };
                format!("• thoughts {duration}")
            }
        }
    }

    fn is_toggleable(&self) -> bool {
        self.has_toggle_header
    }
}

fn reasoning_message_render_cache_key(
    content: &str,
    display_mode: ReasoningDisplayMode,
    has_toggle_header: bool,
    duration: Option<Duration>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "reasoning_message".hash(&mut hasher);
    display_mode.hash(&mut hasher);
    has_toggle_header.hash(&mut hasher);
    duration.hash(&mut hasher);
    if !matches!(display_mode, ReasoningDisplayMode::Snippet) {
        content.hash(&mut hasher);
    }
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
    use crate::{
        styled_text::line_to_plain_text,
        theme::{default_palette, tertiary_text_style},
    };

    #[test]
    fn reasoning_message_renders_expanded_content_without_header() {
        let palette = default_palette();
        let item = ReasoningMessageItem::new("先分析", ReasoningDisplayMode::Expanded, None);
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["先分析".to_string()]
        );
        assert_eq!(lines[0].spans[0].style.fg, tertiary_text_style(palette).fg);
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert!(!item.is_header_line(0));
        assert_eq!(item.header_display_width(), 0);
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
    fn toggled_reasoning_message_header_uses_subsecond_duration_label() {
        let mut item = ReasoningMessageItem::new(
            "先分析",
            ReasoningDisplayMode::Collapsed,
            Some(Duration::from_millis(120)),
        );

        item.toggle();

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

    #[test]
    fn reasoning_message_snippet_renders_only_duration_hint() {
        let item = ReasoningMessageItem::new(
            "这段内容不能进入 transcript",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(16)),
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts 16s".to_string()]
        );
        assert_eq!(item.source_text_byte_len(), 0);
        assert_eq!(item.header_display_width(), 0);
    }

    #[test]
    fn reasoning_message_snippet_uses_subsecond_duration_label() {
        let item = ReasoningMessageItem::new(
            "这段内容不能进入 transcript",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_millis(120)),
        );

        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts <1s".to_string()]
        );
    }

    #[test]
    fn reasoning_message_snippet_does_not_toggle_or_hash_content() {
        let mut item = ReasoningMessageItem::new(
            "第一段思维链",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(3)),
        );
        let same_hint_item = ReasoningMessageItem::new(
            "第二段思维链",
            ReasoningDisplayMode::Snippet,
            Some(Duration::from_secs(3)),
        );
        let original_key = item.render_cache_key();

        item.toggle();

        assert_eq!(item.render_cache_key(), original_key);
        assert_eq!(item.render_cache_key(), same_hint_item.render_cache_key());
        assert_eq!(
            item.render_lines(80, default_palette())
                .iter()
                .map(line_to_plain_text)
                .collect::<Vec<_>>(),
            vec!["• thoughts 3s".to_string()]
        );
    }
}
