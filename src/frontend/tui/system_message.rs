use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::text::Line;

use super::{
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, system_error_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
        wrap_prompt_visual_lines,
    },
};

const SYSTEM_MESSAGE_PREFIX: &str = "■ ";

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
        wrap_prompt_visual_lines(&self.prefixed_content(), usize::from(width.max(1)), 0)
    }

    fn prefixed_content(&self) -> String {
        format!("{SYSTEM_MESSAGE_PREFIX}{}", self.content)
    }
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
    use crate::frontend::tui::{
        styled_text::line_to_plain_text,
        theme::{default_palette, system_error_text_style},
    };

    #[test]
    fn system_message_renders_prefix_and_error_color() {
        let palette = default_palette();
        let item = SystemMessageItem::new("Chat failed: connection refused");
        let lines = item.render_lines(80, palette);

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["■ Chat failed: connection refused".to_string()]
        );
        assert_eq!(lines[0].style, system_error_text_style(palette));
    }
}
