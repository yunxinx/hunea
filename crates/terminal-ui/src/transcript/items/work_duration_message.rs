use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Duration,
};

use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use crate::{
    stream_activity::format_elapsed_compact,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::{TerminalPalette, tertiary_text_style},
    transcript::{
        ItemLineAnchor, TranscriptEstimateKind, TranscriptFastEstimate, TranscriptItemMetrics,
    },
};

const DIVIDER: &str = "─";
const WORK_DURATION_PREFIX: &str = "─ Worked for ";

/// `WorkDurationMessageItem` 表示单轮 assistant 结束后的耗时分割线。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkDurationMessageItem {
    duration: Duration,
    render_cache_key: u64,
}

impl WorkDurationMessageItem {
    /// `new` 创建一条 assistant 工作耗时分割线。
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            render_cache_key: work_duration_message_render_cache_key(duration),
        }
    }

    /// `render_lines` 将耗时提示渲染为占满整行的分割线。
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
        work_duration_line(self.duration, usize::from(width.max(1)))
    }
}

fn work_duration_line(duration: Duration, width: usize) -> String {
    let label = format!(
        "{WORK_DURATION_PREFIX}{} ",
        format_elapsed_compact(duration.as_secs())
    );
    let label_width = label.width();
    if label_width >= width {
        return label.chars().take(width).collect();
    }

    format!("{label}{}", DIVIDER.repeat(width - label_width))
}

fn work_duration_message_render_cache_key(duration: Duration) -> u64 {
    let mut hasher = DefaultHasher::new();
    "work_duration_message".hash(&mut hasher);
    duration.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{styled_text::line_to_plain_text, theme::default_palette};

    #[test]
    fn work_duration_message_fills_available_width() {
        let item = WorkDurationMessageItem::new(Duration::from_secs(65));
        let lines = item.render_lines(24, default_palette());

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["─ Worked for 1m 05s ────".to_string()]
        );
    }

    #[test]
    fn work_duration_message_truncates_when_viewport_is_too_narrow() {
        let item = WorkDurationMessageItem::new(Duration::from_secs(5));
        let lines = item.render_lines(8, default_palette());

        assert_eq!(
            lines.iter().map(line_to_plain_text).collect::<Vec<_>>(),
            vec!["─ Worked".to_string()]
        );
    }
}
