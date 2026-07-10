use std::path::Path;

use ratatui::text::Line;

use crate::{
    styled_text::lines_to_plain_text,
    theme::TerminalPalette,
    transcript::{
        DEFAULT_RENDER_WIDTH, TranscriptEstimateKind, TranscriptEstimateSource,
        TranscriptFastEstimate, TranscriptItemMetrics, estimate_markdown_metrics_for_tabs,
        render_markdown_lines, render_markdown_metrics, wrap_assistant_text,
    },
};

use super::assistant_estimate::{
    estimate_common_markdown_metrics_exact_fast, estimate_common_markdown_metrics_fast,
};

const ASSISTANT_MESSAGE_INSET_WIDTH: usize = 2;

pub(super) fn render_assistant_message(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> Vec<Line<'static>> {
    let width = assistant_message_content_width(width);
    let rendered = render_markdown_lines(content, width, palette, working_dir);
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
    working_dir: Option<&Path>,
) -> (usize, usize) {
    let width = assistant_message_content_width(width);
    if !content.contains('\t')
        && let Some(metrics) = estimate_common_markdown_metrics_exact_fast(content, width)
    {
        return metrics.into_tuple();
    }

    let metrics = render_markdown_metrics(content, width, palette, working_dir);
    if metrics.0 > 0 {
        return metrics;
    }

    let wrapped = wrap_assistant_text(content, width, 0);
    (wrapped.len(), wrapped.iter().map(String::len).sum())
}

pub(super) fn estimate_assistant_message_metrics_fast(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    previous_metrics: Option<TranscriptItemMetrics>,
    working_dir: Option<&Path>,
) -> TranscriptFastEstimate {
    let width = assistant_message_content_width(width);
    let uses_tab_exact_estimate = content.contains('\t');
    let (content_line_count, estimated_char_len) = if uses_tab_exact_estimate {
        let metrics = estimate_markdown_metrics_for_tabs(content, width, palette, working_dir);
        if metrics.0 > 0 {
            metrics
        } else {
            let rendered = render_markdown_lines(content, width, palette, working_dir);
            (rendered.len().max(1), lines_to_plain_text(&rendered).len())
        }
    } else if let Some(metrics) = estimate_common_markdown_metrics_fast(content, width) {
        metrics.into_tuple()
    } else {
        let wrapped = wrap_assistant_text(content, width, 0);
        (
            wrapped.len().max(1),
            wrapped.iter().map(String::len).sum::<usize>(),
        )
    };
    let reused_metrics = (!uses_tab_exact_estimate)
        .then_some(previous_metrics)
        .flatten()
        .filter(|metrics| metrics.is_valid && usize::from(metrics.width) != width);
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

pub(crate) fn assistant_message_visual_inset(width: u16) -> u16 {
    let width = assistant_message_width(width);
    if width < ASSISTANT_MESSAGE_INSET_WIDTH * 4 {
        return 0;
    }

    u16::try_from(ASSISTANT_MESSAGE_INSET_WIDTH).unwrap_or(u16::MAX)
}

pub(crate) fn assistant_message_content_width(width: u16) -> usize {
    let full_width = assistant_message_width(width);
    let inset = usize::from(assistant_message_visual_inset(width));

    full_width.saturating_sub(inset.saturating_mul(2)).max(1)
}

fn assistant_message_width(width: u16) -> usize {
    if width == 0 {
        DEFAULT_RENDER_WIDTH
    } else {
        usize::from(width)
    }
}
