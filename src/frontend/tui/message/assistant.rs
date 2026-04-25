use ratatui::text::Line;

use crate::frontend::tui::{
    theme::TerminalPalette,
    transcript::{
        DEFAULT_RENDER_WIDTH, TranscriptEstimateKind, TranscriptEstimateSource,
        TranscriptFastEstimate, TranscriptItemMetrics, render_markdown_lines,
        render_markdown_metrics, wrap_assistant_text,
    },
};

pub(super) fn render_assistant_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let width = if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    };
    let rendered = render_markdown_lines(content, width, palette);
    if rendered.is_empty() {
        return wrap_assistant_text(content, width, 0)
            .into_iter()
            .map(Line::raw)
            .collect();
    }

    rendered
}

pub(super) fn render_assistant_message_metrics(
    content: &str,
    width: u16,
    palette: TerminalPalette,
) -> (usize, usize) {
    let width = if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    };
    let metrics = render_markdown_metrics(content, width, palette);
    if metrics.0 > 0 {
        return metrics;
    }

    let wrapped = wrap_assistant_text(content, width, 0);
    (wrapped.len(), wrapped.iter().map(String::len).sum())
}

pub(super) fn estimate_assistant_message_metrics_fast(
    content: &str,
    width: u16,
    previous_metrics: Option<TranscriptItemMetrics>,
) -> TranscriptFastEstimate {
    let width = usize::from(width.max(1));
    let wrapped = wrap_assistant_text(content, width, 0);
    let content_line_count = wrapped.len().max(1);
    let estimated_char_len = wrapped.iter().map(String::len).sum::<usize>();
    let reused_metrics =
        previous_metrics.filter(|metrics| metrics.is_valid && usize::from(metrics.width) != width);
    let content_char_len = reused_metrics
        .map(|metrics| metrics.content_char_len.max(estimated_char_len))
        .unwrap_or(estimated_char_len);

    TranscriptFastEstimate {
        content_line_count,
        content_char_len,
        kind: TranscriptEstimateKind::Assistant,
        source: if reused_metrics.is_some() {
            TranscriptEstimateSource::ReusedOnResize
        } else {
            TranscriptEstimateSource::Fresh
        },
    }
}
