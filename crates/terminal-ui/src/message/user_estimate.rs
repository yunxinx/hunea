#[cfg(test)]
use std::cell::Cell;

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    StyleMode,
    display_width::{display_width, grapheme_width},
    terminal_text::sanitize_terminal_text,
    theme::TerminalPalette,
    transcript::{
        ProseWrapOptions, TranscriptEstimateKind, TranscriptEstimateSource, TranscriptFastEstimate,
        TranscriptItemMetrics, WrappedWhitespace, display_tab_width,
        prose_wrap_is_monotone_when_widening, split_text_lines, wrap_prose_ranges,
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

#[cfg(test)]
thread_local! {
    static ESTIMATE_RENDERED_PROMPT_TEXT_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn estimate_user_message_metrics_fast(
    content: &str,
    width: u16,
    palette: TerminalPalette,
    style_mode: StyleMode,
    can_reuse_user_line_count_when_widening: bool,
    previous_metrics: Option<TranscriptItemMetrics>,
) -> TranscriptFastEstimate {
    let width = width.max(1);
    let layout = user_message_estimate_layout(width, style_mode);
    let has_frame = layout.shows_frame && has_visible_user_message_frame(palette);
    let frame_line_count = usize::from(has_frame) * 2;

    let reused_metrics =
        previous_metrics.filter(|metrics| metrics.is_valid && metrics.width != width);
    if let Some(previous_metrics) = reused_metrics {
        let old_frame_line_count = frame_line_count;
        let old_content_line_count = previous_metrics
            .content_line_count
            .saturating_sub(old_frame_line_count)
            .max(1);
        let new_content_width = layout.content_width.max(1);
        // content_width 随终端宽度单调不减，比较原始宽度即可判定变宽方向。
        let widened = width >= previous_metrics.width;
        if widened && can_reuse_user_line_count_when_widening {
            let estimated_char_len = estimate_user_plain_text_len_on_widening(
                previous_metrics,
                content.len(),
                layout,
                style_mode,
                has_frame,
                old_content_line_count,
            );
            return TranscriptFastEstimate {
                content_line_count: old_content_line_count.saturating_add(frame_line_count),
                content_char_len: previous_metrics.content_char_len.max(estimated_char_len),
                kind: TranscriptEstimateKind::NonAssistant,
                source: TranscriptEstimateSource::ReusedOnResize,
            };
        }

        // 变窄，或内容含 URL 原子保护 / 短缩进 / 宽空白等非单调特征时，
        // 旧行数只能作为保守下界，需按当前宽度重新估算。
        let estimated_content_line_count = estimate_wrapped_line_count_by_display_width(
            content,
            new_content_width,
            layout.line_prefix_width,
        )
        .max(old_content_line_count);

        let content_line_count = estimated_content_line_count.saturating_add(frame_line_count);
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
            source: TranscriptEstimateSource::Fresh,
        };
    }

    let estimated_content_line_count = estimate_wrapped_line_count_by_display_width(
        content,
        layout.content_width.max(1),
        layout.line_prefix_width,
    );
    let content_line_count = estimated_content_line_count.saturating_add(frame_line_count);
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

fn user_message_estimate_layout(width: u16, style_mode: StyleMode) -> UserMessageRenderLayout {
    let width = width.max(1);
    match style_mode.normalized() {
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
    }
}

/// 判断 user prompt 是否可以在宽度变大时安全复用旧行数。
///
/// certificate 在消息构造期计算一次；resize 热路径只读取布尔值。任意前导
/// 空格与 hard-wrap 控制字符都保守拒绝，避免把 prompt 特有的前缀回退规则
/// 混入 prose planner 的单调性证明。
pub(super) fn can_reuse_user_line_count_when_widening(content: &str) -> bool {
    let sanitized = sanitize_terminal_text(content);
    let content = sanitized.as_ref();
    prose_wrap_is_monotone_when_widening(content)
        && split_text_lines(content).all(|(line, _)| {
            !line
                .chars()
                .next()
                .is_some_and(|character| character == ' ')
        })
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
    for (raw_line, _) in split_text_lines(content) {
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

        let (estimated_line_count, first_line_width) =
            estimate_prompt_prose(remainder, first_width, content_width);
        if leading_spaces.saturating_add(first_line_width) > content_width {
            return 1 + estimate_prompt_prose_line_count(remainder, content_width, content_width);
        }

        return estimated_line_count;
    }

    estimate_prompt_prose_line_count(raw_line, content_width, content_width)
}

fn estimate_prompt_prose_line_count(
    text: &str,
    first_width: usize,
    continuation_width: usize,
) -> usize {
    estimate_prompt_prose(text, first_width, continuation_width).0
}

fn estimate_prompt_prose(
    text: &str,
    first_width: usize,
    continuation_width: usize,
) -> (usize, usize) {
    let wrapped = wrap_prose_ranges(
        text,
        ProseWrapOptions {
            first_width,
            continuation_width,
            wrapped_whitespace: WrappedWhitespace::PreserveMultiple,
            trim_trailing_whitespace: false,
        },
    );
    let first_line_width = wrapped
        .first()
        .map(|line| display_width(&text[line.visible.clone()]))
        .unwrap_or(0);
    (wrapped.len().max(1), first_line_width)
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
            let total_trailing_capacity = layout
                .frame_width
                .max(1)
                .saturating_mul(content_line_count)
                .saturating_sub(prefix_width);
            let line_content_capacity = layout
                .frame_width
                .max(1)
                .saturating_sub(layout.line_prefix_width);
            // 通常每条视觉行都不超过 trailing-fill 容量，因此总 fill 可由总显示宽度
            // 精确扣除。超宽 grapheme 与 tab 会让某一行溢出；此时不能用该溢出抵消
            // 其它行仍真实存在的 fill，退回容量总和作为保守上界。
            let occupied_width = if rendered_text.has_tab
                || rendered_text.max_grapheme_width > line_content_capacity
            {
                0
            } else {
                rendered_text.display_width
            };
            let trailing_fill_len = total_trailing_capacity.saturating_sub(occupied_width);
            text_with_prefixes.saturating_add(trailing_fill_len)
        }
        StyleMode::Ms => text_with_prefixes,
    };

    plain_text_len += estimated_line_len;
    plain_text_len
}

fn estimate_user_plain_text_len_on_widening(
    previous_metrics: TranscriptItemMetrics,
    content_byte_len: usize,
    layout: UserMessageRenderLayout,
    style_mode: StyleMode,
    has_frame: bool,
    content_line_count: usize,
) -> usize {
    let previous_layout = user_message_estimate_layout(previous_metrics.width, style_mode);
    let frame_width_delta = layout
        .frame_width
        .saturating_sub(previous_layout.frame_width);

    // certificate 保证内容行不会增加。Cx/Cc 的普通空格可见性变化会被
    // trailing fill 抵消，每个保留行最多增长 frame_width_delta；Ms 没有 fill，
    // 需用原始 byte length 加旧行数前缀建立不扫描正文的保守上界。
    let content_char_len = match style_mode.normalized() {
        StyleMode::Cx | StyleMode::Cc => previous_metrics
            .content_char_len
            .saturating_add(content_line_count.saturating_mul(frame_width_delta)),
        StyleMode::Ms => content_byte_len
            .saturating_add(user_message_prefix(style_mode).len())
            .saturating_add(
                content_line_count
                    .saturating_sub(1)
                    .saturating_mul(layout.line_prefix_width),
            ),
    };
    let frame_fill_delta = usize::from(has_frame)
        .saturating_mul(2)
        .saturating_mul(frame_width_delta);

    content_char_len.saturating_add(frame_fill_delta)
}

#[derive(Debug, Clone, Copy, Default)]
struct PromptTextEstimate {
    byte_len: usize,
    display_width: usize,
    max_grapheme_width: usize,
    has_tab: bool,
}

fn estimate_rendered_prompt_text(content: &str) -> PromptTextEstimate {
    #[cfg(test)]
    ESTIMATE_RENDERED_PROMPT_TEXT_CALL_COUNT.with(|count| count.set(count.get() + 1));

    let mut estimate = PromptTextEstimate::default();
    for cluster in UnicodeSegmentation::graphemes(content, true) {
        if cluster == "\n" {
            continue;
        }
        if cluster == "\t" {
            let width = display_tab_width(0);
            estimate.byte_len = estimate.byte_len.saturating_add(width);
            estimate.display_width = estimate.display_width.saturating_add(width);
            estimate.max_grapheme_width = estimate.max_grapheme_width.max(width);
            estimate.has_tab = true;
        } else {
            let width = grapheme_width(cluster);
            estimate.byte_len = estimate.byte_len.saturating_add(cluster.len());
            estimate.display_width = estimate.display_width.saturating_add(width);
            estimate.max_grapheme_width = estimate.max_grapheme_width.max(width);
        }
    }
    estimate
}

#[cfg(test)]
pub(super) fn reset_estimate_rendered_prompt_text_call_count() {
    ESTIMATE_RENDERED_PROMPT_TEXT_CALL_COUNT.set(0);
}

#[cfg(test)]
pub(super) fn estimate_rendered_prompt_text_call_count() -> usize {
    ESTIMATE_RENDERED_PROMPT_TEXT_CALL_COUNT.get()
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
