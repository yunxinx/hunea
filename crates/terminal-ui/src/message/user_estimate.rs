use unicode_segmentation::UnicodeSegmentation;

use crate::{
    StyleMode,
    display_width::grapheme_width,
    theme::TerminalPalette,
    transcript::{
        TranscriptEstimateKind, TranscriptEstimateSource, TranscriptFastEstimate,
        TranscriptItemMetrics, WrapSegmentKind, display_tab_width, should_start_new_wrap_segment,
        wrap_segment_kind,
    },
};

use super::{
    UserMessageRenderLayout,
    user::{
        compact_user_plain_line_len, framed_user_plain_line_len, has_visible_user_message_frame,
        legacy_user_plain_line_len, user_message_compact_content_width, user_message_inset_width,
        user_message_layout, user_message_legacy_content_width, user_message_prefix,
        user_message_prefix_glyph,
    },
    user_projection::user_message_wrap_snapshot,
};

pub(super) fn estimate_user_message_metrics_fast(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
    previous_metrics: Option<TranscriptItemMetrics>,
) -> TranscriptFastEstimate {
    let width = width.max(1);
    let layout = match style_mode.normalized() {
        StyleMode::Ms => UserMessageRenderLayout {
            frame_width: usize::from(width),
            content_width: user_message_legacy_content_width(width, style_mode),
            line_prefix_width: user_message_inset_width(style_mode),
            shows_prefix: true,
            shows_frame: false,
        },
        StyleMode::Cc => UserMessageRenderLayout {
            frame_width: usize::from(width),
            content_width: user_message_compact_content_width(width, style_mode),
            line_prefix_width: user_message_inset_width(style_mode),
            shows_prefix: true,
            shows_frame: false,
        },
        StyleMode::Cx => user_message_layout(width, style_mode),
    };
    let has_frame = layout.shows_frame && has_visible_user_message_frame(palette);
    let frame_line_count = usize::from(has_frame) * 2;

    let reused_metrics =
        previous_metrics.filter(|metrics| metrics.is_valid && metrics.width != width);
    if let Some(previous_metrics) = reused_metrics {
        let previous_layout = match style_mode.normalized() {
            StyleMode::Ms => UserMessageRenderLayout {
                frame_width: usize::from(previous_metrics.width.max(1)),
                content_width: user_message_legacy_content_width(
                    previous_metrics.width,
                    style_mode,
                ),
                line_prefix_width: user_message_inset_width(style_mode),
                shows_prefix: true,
                shows_frame: false,
            },
            StyleMode::Cc => UserMessageRenderLayout {
                frame_width: usize::from(previous_metrics.width.max(1)),
                content_width: user_message_compact_content_width(
                    previous_metrics.width,
                    style_mode,
                ),
                line_prefix_width: user_message_inset_width(style_mode),
                shows_prefix: true,
                shows_frame: false,
            },
            StyleMode::Cx => user_message_layout(previous_metrics.width.max(1), style_mode),
        };
        let old_frame_line_count = frame_line_count;
        let old_content_line_count = previous_metrics
            .content_line_count
            .saturating_sub(old_frame_line_count)
            .max(1);

        let old_content_width = previous_layout.content_width.max(1);
        let new_content_width = layout.content_width.max(1);
        let (estimated_content_line_count, source) = if new_content_width >= old_content_width {
            (
                old_content_line_count,
                TranscriptEstimateSource::ReusedOnResize,
            )
        } else {
            (
                estimate_wrapped_line_count_by_display_width(
                    content,
                    new_content_width,
                    layout.line_prefix_width,
                )
                .max(old_content_line_count),
                TranscriptEstimateSource::Fresh,
            )
        };

        let content_line_count = estimated_content_line_count + frame_line_count;
        let estimated_char_len = estimate_user_plain_text_len_fast(
            content,
            layout,
            style_mode,
            has_frame,
            estimated_content_line_count,
        );
        let content_char_len = previous_metrics.content_char_len.max(estimated_char_len);

        return TranscriptFastEstimate {
            content_line_count,
            content_char_len,
            kind: TranscriptEstimateKind::NonAssistant,
            source,
        };
    }

    let estimated_content_line_count = estimate_wrapped_line_count_by_display_width(
        content,
        layout.content_width.max(1),
        layout.line_prefix_width,
    );
    let content_line_count = estimated_content_line_count + frame_line_count;
    let content_char_len = estimate_user_plain_text_len_fast(
        content,
        layout,
        style_mode,
        has_frame,
        estimated_content_line_count,
    );

    TranscriptFastEstimate {
        content_line_count,
        content_char_len,
        kind: TranscriptEstimateKind::NonAssistant,
        source: TranscriptEstimateSource::Fresh,
    }
}

pub(super) fn estimate_wrapped_line_count_by_display_width(
    content: &str,
    content_width: usize,
    line_prefix_width: usize,
) -> usize {
    let content_width = content_width.max(1);
    if content.is_empty() {
        return 1;
    }

    let mut total = 0usize;
    for raw_line in content.split('\n') {
        total += estimate_prompt_logical_line_count(raw_line, content_width, line_prefix_width);
    }

    total.max(1)
}

fn estimate_prompt_logical_line_count(
    raw_line: &str,
    content_width: usize,
    line_prefix_width: usize,
) -> usize {
    let content_width = content_width.max(1);
    if raw_line.is_empty() {
        return 1;
    }

    let leading_spaces = raw_line.chars().take_while(|ch| *ch == ' ').count();
    if raw_line.contains('\t') {
        return estimate_hard_wrap_line_count(raw_line, content_width, line_prefix_width);
    }
    if leading_spaces >= 4 {
        return estimate_hard_wrap_line_count(raw_line, content_width, line_prefix_width);
    }

    if leading_spaces > 0 && leading_spaces < 4 {
        if leading_spaces >= content_width {
            return estimate_hard_wrap_line_count(raw_line, content_width, line_prefix_width);
        }

        let remainder = raw_line
            .char_indices()
            .nth(leading_spaces)
            .map(|(byte_index, _)| &raw_line[byte_index..])
            .unwrap_or("");
        let first_width = content_width.saturating_sub(leading_spaces).max(1);

        if short_indent_requires_prefix_only_fallback(remainder, first_width) {
            return 1 + estimate_prompt_prose_line_count(remainder, content_width, content_width);
        }

        return estimate_prompt_prose_line_count(remainder, first_width, content_width);
    }

    estimate_prompt_prose_line_count(raw_line, content_width, content_width)
}

fn estimate_prompt_prose_line_count(
    text: &str,
    first_width: usize,
    continuation_width: usize,
) -> usize {
    let first_width = first_width.max(1);
    let continuation_width = continuation_width.max(1);
    if text.is_empty() {
        return 1;
    }

    #[derive(Clone, Copy)]
    struct Segment<'a> {
        text: &'a str,
        width: usize,
        is_space: bool,
    }

    #[derive(Clone, Copy)]
    struct WordBlock<'a> {
        leading_spaces: usize,
        word: Segment<'a>,
        trailing_spaces: usize,
        has_leading_space: bool,
        has_trailing: bool,
    }

    impl WordBlock<'_> {
        fn visible_width(&self, at_line_start: bool) -> usize {
            let mut width = self.word.width;
            if self.has_leading_space && !(at_line_start && self.leading_spaces == 1) {
                width += self.leading_spaces;
            }
            if self.has_trailing {
                width += self.trailing_spaces;
            }
            width
        }

        fn visible_leading_spaces(&self, at_line_start: bool) -> usize {
            if self.has_leading_space && !(at_line_start && self.leading_spaces == 1) {
                self.leading_spaces
            } else {
                0
            }
        }
    }

    let mut segments = Vec::new();
    let mut current_start = 0usize;
    let mut current_width = 0usize;
    let mut current_kind = None;

    for (start, cluster) in UnicodeSegmentation::grapheme_indices(text, true) {
        let kind = wrap_segment_kind(cluster);
        if current_kind.is_none() {
            current_start = start;
            current_kind = Some(kind);
        } else if current_kind.is_some_and(|existing| should_start_new_wrap_segment(existing, kind))
        {
            segments.push(Segment {
                text: &text[current_start..start],
                width: current_width,
                is_space: current_kind == Some(WrapSegmentKind::Space),
            });
            current_start = start;
            current_width = 0;
            current_kind = Some(kind);
        }

        current_width = current_width.saturating_add(match cluster {
            // estimated path 把 tab 当作固定 8 列宽的 stop；可见窗口 exactize 会纠正细节。
            "\t" => 8,
            _ => grapheme_width(cluster),
        });
    }

    if let Some(kind) = current_kind {
        segments.push(Segment {
            text: &text[current_start..],
            width: current_width,
            is_space: kind == WrapSegmentKind::Space,
        });
    }

    if segments.is_empty() {
        return 1;
    }

    let mut blocks = Vec::with_capacity(segments.len() / 2 + 1);
    let mut pending_spaces = 0usize;
    let mut has_pending_spaces = false;
    let mut index = 0usize;

    while index < segments.len() {
        let segment = segments[index];
        if segment.is_space {
            pending_spaces = segment.width;
            has_pending_spaces = true;
            index += 1;
            continue;
        }

        let mut block = WordBlock {
            leading_spaces: pending_spaces,
            word: segment,
            trailing_spaces: 0,
            has_leading_space: has_pending_spaces,
            has_trailing: false,
        };
        pending_spaces = 0;
        has_pending_spaces = false;

        if index + 1 < segments.len() && segments[index + 1].is_space && index + 2 >= segments.len()
        {
            block.trailing_spaces = segments[index + 1].width;
            block.has_trailing = true;
            index += 1;
        }

        blocks.push(block);
        index += 1;
    }

    if blocks.is_empty() {
        return estimate_hard_wrap_width_line_count(
            segments
                .iter()
                .map(|segment| segment.width)
                .sum::<usize>()
                .max(1),
            first_width,
            continuation_width,
        );
    }

    let mut line_count = 1usize;
    let mut current_limit = first_width;
    let mut current_width_used = 0usize;
    let mut has_content = false;

    let should_reflow_exact_fit_block =
        |current_width_used: usize,
         current_limit: usize,
         continuation_width: usize,
         block: &WordBlock<'_>,
         remaining: &[WordBlock<'_>]| {
            if current_width_used == 0 || continuation_width == 0 || remaining.is_empty() {
                return false;
            }
            if current_width_used + block.visible_width(false) != current_limit {
                return false;
            }

            let next_block = remaining[0];
            if !next_block.has_leading_space || next_block.leading_spaces <= 1 {
                return false;
            }

            // 这里要与 exact wrapper 的 reflow 规则保持一致，避免多空格前缀被错误吞进上一行。
            block.visible_width(true) + next_block.visible_width(false) <= continuation_width
        };

    for (index, block) in blocks.iter().enumerate() {
        loop {
            let at_line_start = !has_content;
            let block_width = block.visible_width(at_line_start).max(1);

            if current_width_used + block_width <= current_limit {
                if !at_line_start
                    && should_reflow_exact_fit_block(
                        current_width_used,
                        current_limit,
                        continuation_width,
                        block,
                        &blocks[index + 1..],
                    )
                {
                    line_count += 1;
                    current_limit = continuation_width;
                    current_width_used = 0;
                    has_content = false;
                    continue;
                }

                current_width_used += block_width;
                has_content = true;
                break;
            }

            if !at_line_start {
                line_count += 1;
                current_limit = continuation_width;
                current_width_used = 0;
                has_content = false;
                continue;
            }

            let leading_spaces = block.visible_leading_spaces(true);
            let trailing_spaces = if block.has_trailing {
                block.trailing_spaces
            } else {
                0
            };
            let (lines_used, last_line_width) = estimate_hard_wrap_word_block(
                leading_spaces,
                block.word.text,
                trailing_spaces,
                current_limit,
                continuation_width,
            );
            line_count += lines_used.saturating_sub(1);
            current_limit = continuation_width;
            current_width_used = last_line_width;
            has_content = true;
            break;
        }
    }

    line_count.max(1)
}

fn short_indent_requires_prefix_only_fallback(remainder: &str, first_width: usize) -> bool {
    UnicodeSegmentation::graphemes(remainder, true)
        .next()
        .map(|cluster| grapheme_width(cluster) > first_width.max(1))
        .unwrap_or(false)
}

pub(super) fn estimate_hard_wrap_line_count(
    raw_line: &str,
    width: usize,
    line_prefix_width: usize,
) -> usize {
    let width = width.max(1);
    if raw_line.is_empty() {
        return 1;
    }

    let (lines_used, _) =
        estimate_hard_wrap_visible_text(raw_line, width, width, line_prefix_width);
    lines_used.max(1)
}

pub(super) fn estimate_hard_wrap_visible_text(
    text: &str,
    first_width: usize,
    continuation_width: usize,
    line_prefix_width: usize,
) -> (usize, usize) {
    let first_width = first_width.max(1);
    let continuation_width = continuation_width.max(1);
    if text.is_empty() {
        return (1, 0);
    }

    let mut line_count = 1usize;
    let mut current_limit = first_width;
    let mut current_width_used = 0usize;
    let mut current_has_content = false;

    for cluster in UnicodeSegmentation::graphemes(text, true) {
        if current_width_used >= current_limit && current_width_used > 0 {
            line_count = line_count.saturating_add(1);
            current_limit = continuation_width;
            current_width_used = 0;
            current_has_content = false;
        }

        if cluster == "\t" {
            // tab 会先按当前绝对列展开成空格，再把这段可见宽度继续分摊到后续 continuation line。
            let mut remaining_tab_width = display_tab_width(line_prefix_width + current_width_used);
            while remaining_tab_width > 0 {
                let available_width = current_limit.saturating_sub(current_width_used);
                if available_width == 0 {
                    line_count = line_count.saturating_add(1);
                    current_limit = continuation_width;
                    current_width_used = 0;
                    current_has_content = false;
                    continue;
                }

                let fitted_width = remaining_tab_width.min(available_width);
                current_width_used = current_width_used.saturating_add(fitted_width);
                current_has_content = true;
                remaining_tab_width = remaining_tab_width.saturating_sub(fitted_width);

                if remaining_tab_width > 0 {
                    line_count = line_count.saturating_add(1);
                    current_limit = continuation_width;
                    current_width_used = 0;
                    current_has_content = false;
                }
            }
            continue;
        }

        let cluster_width = grapheme_width(cluster);
        if current_width_used.saturating_add(cluster_width) > current_limit && current_has_content {
            line_count = line_count.saturating_add(1);
            current_limit = continuation_width;
            current_width_used = 0;
        }
        current_width_used = current_width_used.saturating_add(cluster_width);
        current_has_content = true;
    }

    (line_count.max(1), current_width_used)
}

fn estimate_hard_wrap_word_block(
    leading_spaces: usize,
    word: &str,
    trailing_spaces: usize,
    first_width: usize,
    continuation_width: usize,
) -> (usize, usize) {
    let first_width = first_width.max(1);
    let continuation_width = continuation_width.max(1);
    if leading_spaces == 0 && word.is_empty() && trailing_spaces == 0 {
        return (1, 0);
    }

    let mut line_count = 1usize;
    let mut current_limit = first_width;
    let mut current_width_used = 0usize;
    let mut current_has_content = false;

    let mut push_width = |cluster_width: usize| {
        if current_width_used.saturating_add(cluster_width) > current_limit && current_has_content {
            line_count = line_count.saturating_add(1);
            current_limit = continuation_width;
            current_width_used = 0;
            current_has_content = false;
        }

        current_width_used = current_width_used.saturating_add(cluster_width);
        current_has_content = true;
    };

    for _ in 0..leading_spaces {
        push_width(1);
    }

    for cluster in UnicodeSegmentation::graphemes(word, true) {
        let cluster_width = match cluster {
            // estimated path 把 tab 当作固定 8 列宽的 stop；可见窗口 exactize 会纠正细节。
            "\t" => 8,
            _ => grapheme_width(cluster),
        };
        push_width(cluster_width);
    }

    for _ in 0..trailing_spaces {
        push_width(1);
    }

    (line_count.max(1), current_width_used)
}

fn estimate_hard_wrap_width(
    width: usize,
    first_width: usize,
    continuation_width: usize,
) -> (usize, usize) {
    let first_width = first_width.max(1);
    let continuation_width = continuation_width.max(1);
    if width <= first_width {
        return (1, width);
    }

    let remaining = width.saturating_sub(first_width);
    let additional_lines = remaining.div_ceil(continuation_width);
    let total_lines = additional_lines.saturating_add(1);
    let remainder = remaining % continuation_width;
    let last_width = if remainder == 0 {
        continuation_width
    } else {
        remainder
    };

    (total_lines.max(1), last_width)
}

fn estimate_hard_wrap_width_line_count(
    width: usize,
    first_width: usize,
    continuation_width: usize,
) -> usize {
    estimate_hard_wrap_width(width, first_width, continuation_width)
        .0
        .max(1)
}

fn estimate_user_plain_text_len_fast(
    content: &str,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
    has_frame: bool,
    content_line_count: usize,
) -> usize {
    let content_line_count = content_line_count.max(1);
    let mut plain_text_len = usize::from(has_frame) * 2 * layout.frame_width.max(1);
    let rendered_text = estimate_rendered_prompt_text(content);

    let first_prefix_len = match style_mode.normalized() {
        StyleMode::Cx | StyleMode::Cc => user_message_prefix_glyph(style_mode).len() + 1,
        StyleMode::Ms => user_message_prefix(style_mode).len(),
    };
    let continuation_prefix_len = layout.line_prefix_width;
    let prefix_text_len = first_prefix_len
        + content_line_count
            .saturating_sub(1)
            .saturating_mul(continuation_prefix_len);
    let text_with_prefixes = rendered_text.byte_len.saturating_add(prefix_text_len);

    let estimated_line_len = match style_mode.normalized() {
        StyleMode::Cx | StyleMode::Cc => {
            let prefix_width = layout.line_prefix_width.saturating_mul(content_line_count);
            let trailing_fill_len = layout
                .frame_width
                .max(1)
                .saturating_mul(content_line_count)
                .saturating_sub(prefix_width.saturating_add(rendered_text.display_width));
            text_with_prefixes.saturating_add(trailing_fill_len)
        }
        StyleMode::Ms => text_with_prefixes,
    };

    plain_text_len += estimated_line_len;
    plain_text_len
}

#[derive(Debug, Clone, Copy, Default)]
struct PromptTextEstimate {
    byte_len: usize,
    display_width: usize,
}

fn estimate_rendered_prompt_text(content: &str) -> PromptTextEstimate {
    let mut estimate = PromptTextEstimate::default();
    for cluster in UnicodeSegmentation::graphemes(content, true) {
        if cluster == "\n" {
            continue;
        }
        if cluster == "\t" {
            let width = display_tab_width(0);
            estimate.byte_len = estimate.byte_len.saturating_add(width);
            estimate.display_width = estimate.display_width.saturating_add(width);
        } else {
            estimate.byte_len = estimate.byte_len.saturating_add(cluster.len());
            estimate.display_width = estimate
                .display_width
                .saturating_add(grapheme_width(cluster));
        }
    }
    estimate
}

pub(super) fn measure_user_message_metrics(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
) -> (usize, usize) {
    let snapshot = user_message_wrap_snapshot(content, width, palette, style_mode);
    let line_count = snapshot.lines.len() + usize::from(snapshot.has_frame) * 2;
    let mut plain_text_char_len =
        usize::from(snapshot.has_frame) * 2 * snapshot.layout.frame_width.max(1);

    for (index, line) in snapshot.lines.iter().enumerate() {
        let is_first = index == 0;
        plain_text_char_len += match style_mode.normalized() {
            StyleMode::Cx => {
                framed_user_plain_line_len(&line.text, is_first, snapshot.layout, style_mode)
            }
            StyleMode::Cc => compact_user_plain_line_len(
                &line.text,
                is_first,
                snapshot.layout.frame_width,
                style_mode,
            ),
            StyleMode::Ms => legacy_user_plain_line_len(&line.text, is_first, style_mode),
        };
    }

    (line_count, plain_text_char_len)
}
