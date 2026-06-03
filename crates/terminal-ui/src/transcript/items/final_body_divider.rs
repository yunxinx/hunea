use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::text::Line;

use crate::{
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, tertiary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
    },
};

const DIVIDER: &str = "─";

/// `FinalBodyDividerItem` 表示工具活动与最终正文之间的纯分割线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalBodyDividerItem {
    render_cache_key: u64,
}

impl FinalBodyDividerItem {
    /// `new` 创建一条纯分割线。
    pub fn new() -> Self {
        Self {
            render_cache_key: final_body_divider_render_cache_key(),
        }
    }

    /// `render_lines` 将分割线渲染为占满整行的内容。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        vec![Line::styled(
            self.plain_line(width),
            tertiary_text_style(palette),
        )]
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
        0
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        _palette: TerminalPalette,
    ) -> (usize, usize) {
        (1, self.plain_line(width).len())
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

    fn plain_line(&self, width: u16) -> String {
        DIVIDER.repeat(usize::from(width.max(1)))
    }
}

impl Default for FinalBodyDividerItem {
    fn default() -> Self {
        Self::new()
    }
}

fn final_body_divider_render_cache_key() -> u64 {
    let mut hasher = DefaultHasher::new();
    "final_body_divider".hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{styled_text::line_to_plain_text, theme::default_palette};

    #[test]
    fn final_body_divider_fills_available_width() {
        let item = FinalBodyDividerItem::new();
        let lines = item.render_lines(12, default_palette());

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["────────────".to_string()]
        );
    }
}
