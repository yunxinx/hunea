use super::*;

use crate::{
    selection::SelectableLineRange,
    theme::{default_palette, secondary_text_style, surface_text_style},
    transcript::{
        CachedLineAnchors, CachedRenderBlock, prompt_text_wrap_call_count,
        reset_prompt_text_wrap_call_count,
    },
};
use std::rc::Rc;

#[test]
fn assistant_plain_output_preserves_the_raw_command_text() {
    let item = MessageItem::new(Sender::Assistant, "go test ./...");

    assert_eq!(
        item.render_plain_text(6, default_palette()),
        "go\ntest\n./..."
    );
}

#[test]
fn legacy_user_render_wraps_prose_at_word_boundaries() {
    let item = MessageItem::new_with_style_mode(Sender::User, "hello world", StyleMode::Ms);

    let lines = item
        .render_lines(8, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["> hello", "  world"]);
}

#[test]
fn cx_user_render_adds_surface_padding_lines() {
    let palette = default_palette();
    let item = MessageItem::new(Sender::User, "hello");
    let lines = item.render_lines(20, palette);

    assert_eq!(lines.len(), 3);
    assert_eq!(plain_line(lines[0].clone()), "                    ");
    assert_eq!(plain_line(lines[1].clone()), "› hello             ");
    assert_eq!(plain_line(lines[2].clone()), "                    ");
    assert_eq!(lines[1].width(), 20);
    assert_eq!(lines[1].spans.len(), 4);
    assert_eq!(
        lines[1].spans[0].style,
        secondary_text_style(palette).bg(palette.surface.unwrap())
    );
    assert_eq!(lines[1].spans[1].style, surface_text_style(palette));
    assert_eq!(lines[1].spans[2].style, surface_text_style(palette));
    assert_eq!(lines[1].spans[3].style, surface_text_style(palette));
}

#[test]
fn cc_user_terminal_replay_keeps_compact_prefix() {
    let item = MessageItem::new_with_style_mode(Sender::User, "hello", StyleMode::Cc);

    assert_eq!(
        item.render_for_terminal_replay(20, default_palette(), false),
        "❯ hello             "
    );
}

#[test]
fn legacy_user_render_preserves_wrapped_continuation_spaces() {
    let item =
        MessageItem::new_with_style_mode(Sender::User, "aaaaaaaaaaaaaaaaaa b   c", StyleMode::Ms);

    assert_eq!(
        item.render_plain_for_test(22),
        "> aaaaaaaaaaaaaaaaaa\n  b   c"
    );
}

#[test]
fn legacy_user_render_preserves_long_wrapped_leading_spaces() {
    let item = MessageItem::new_with_style_mode(Sender::User, "abc d    e", StyleMode::Ms);

    assert_eq!(item.render_plain_for_test(7), "> abc d\n      e");
}

#[test]
fn assistant_render_keeps_inset_out_of_plain_message_lines() {
    let item = MessageItem::new(Sender::Assistant, "hello world");

    let lines = item
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["hello world"]);
}

#[test]
fn assistant_render_wraps_to_visual_content_width() {
    let item = MessageItem::new(Sender::Assistant, "abcdefghijklmnopqrstuvwxyz");

    let lines = item
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["abcdefghijklmnop", "qrstuvwxyz"]);
}

#[test]
fn assistant_markdown_table_uses_visual_content_width() {
    let item = MessageItem::new(
        Sender::Assistant,
        "| Name | Status |\n| --- | --- |\n| alpha beta gamma | delta epsilon zeta |",
    );

    let lines = item.render_lines(28, default_palette());

    assert!(
        lines.iter().all(|line| line.width() <= 24),
        "table rows should fit the inset content width: {:?}",
        lines.into_iter().map(plain_line).collect::<Vec<_>>()
    );
}

#[test]
fn assistant_render_wraps_leading_make_explanation_as_prose() {
    let item = MessageItem::new(Sender::Assistant, "make the handler return early");

    let lines = item
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["make the handler", "return early"]);
}

#[test]
fn assistant_render_uses_markdown_heading_rendering() {
    let item = MessageItem::new(Sender::Assistant, "# Overview of the API");

    let lines = item
        .render_lines(40, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["# Overview of the API"]);
}

#[test]
fn assistant_render_uses_markdown_emphasis_rendering() {
    let item = MessageItem::new(Sender::Assistant, "__init__");

    let lines = item
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["init"]);
}

#[test]
fn assistant_terminal_replay_matches_screen_markdown_text() {
    let item = MessageItem::new(Sender::Assistant, "## Summary\n\n__init__");

    let screen = item
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        screen,
        item.render_for_terminal_replay(20, default_palette(), false)
    );
}

#[test]
fn user_message_selectable_ranges_skip_prompt_only_leading_blank_line() {
    let item = MessageItem::new(Sender::User, "\nfoo");

    let ranges = item.render_selectable_line_ranges(20, default_palette());

    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0], SelectableLineRange::default());
    assert_eq!(ranges[1], SelectableLineRange::blank_hit_range(0, 20));
    assert_eq!(ranges[2], SelectableLineRange::with_hit_range(2, 5, 0, 5));
    assert_eq!(ranges[3], SelectableLineRange::default());
}

#[test]
fn user_message_selectable_ranges_ignore_trailing_fill() {
    let item = MessageItem::new(Sender::User, "hi");

    let ranges = item.render_selectable_line_ranges(10, default_palette());

    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[1], SelectableLineRange::with_hit_range(2, 4, 0, 4));
}

#[test]
fn user_render_projection_stays_smaller_than_eager_styled_line_cache_for_long_prose() {
    let palette = default_palette();
    let width = 80;
    let item = MessageItem::new_with_style_mode(
        Sender::User,
        "This projection should stay lightweight even when the user message wraps across many lines with ordinary prose content that would otherwise keep a full per-column cursor map alive in the render cache. ".repeat(8),
        StyleMode::Cx,
    );

    let projection = item
        .render_projection(width, palette)
        .expect("user messages should produce a render projection");
    let eager_lines = item.render_lines(width, palette);
    let eager_line_count = eager_lines.len();

    let projected_block = CachedRenderBlock {
        cache_key: 0,
        width,
        palette,
        lines: Rc::new(Vec::new()),
        projected_user: Some(Rc::new(projection)),
        projected_assistant: None,
        line_count: eager_line_count,
        plain_line_byte_lens: Rc::new(Vec::new()),
        anchors: CachedLineAnchors::default(),
        plain_text_char_len: 0,
    };
    let eager_block = CachedRenderBlock {
        cache_key: 0,
        width,
        palette,
        lines: Rc::new(eager_lines),
        projected_user: None,
        projected_assistant: None,
        line_count: eager_line_count,
        plain_line_byte_lens: Rc::new(Vec::new()),
        anchors: CachedLineAnchors::default(),
        plain_text_char_len: 0,
    };

    assert!(
        projected_block.estimated_render_ui_bytes() < eager_block.estimated_render_ui_bytes(),
        "projected user cache should stay smaller than the old eager styled-line cache for long prose messages"
    );
}

#[test]
fn assistant_render_projection_stays_smaller_than_eager_styled_line_cache_for_common_markdown() {
    let palette = default_palette();
    let width = 80;
    let mut markdown = String::from("# Assistant projection\n\n");
    for index in 0..40 {
        markdown.push_str(&format!(
            "## Section {index}\n\n- cached blocks keep only the index\n- styled lines materialize by projection page\n\n```rust\nlet section_{index} = \"ordinary fenced code\";\n```\n\nThe surrounding prose remains Markdown-rendered while avoiding a full eager styled-line cache.\n\n"
        ));
    }
    let item = MessageItem::new(Sender::Assistant, markdown);

    let projection = item
        .render_assistant_projection(width, palette)
        .expect("common long assistant Markdown should produce a render projection");
    let eager_lines = item.render_lines(width, palette);
    let eager_line_count = eager_lines.len();

    let projected_block = CachedRenderBlock {
        cache_key: 0,
        width,
        palette,
        lines: Rc::new(Vec::new()),
        projected_user: None,
        projected_assistant: Some(Rc::new(projection)),
        line_count: eager_line_count,
        plain_line_byte_lens: Rc::new(Vec::new()),
        anchors: CachedLineAnchors::default(),
        plain_text_char_len: 0,
    };
    let eager_block = CachedRenderBlock {
        cache_key: 0,
        width,
        palette,
        lines: Rc::new(eager_lines),
        projected_user: None,
        projected_assistant: None,
        line_count: eager_line_count,
        plain_line_byte_lens: Rc::new(Vec::new()),
        anchors: CachedLineAnchors::default(),
        plain_text_char_len: 0,
    };

    assert!(
        projected_block.estimated_render_ui_bytes() < eager_block.estimated_render_ui_bytes(),
        "projected assistant cache should stay smaller than the old eager styled-line cache for common long Markdown: projected={}, eager={}",
        projected_block.estimated_render_ui_bytes(),
        eager_block.estimated_render_ui_bytes()
    );
}

#[test]
fn user_fast_estimate_stays_conservative_for_basic_wrapping_cases() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(
        Sender::User,
        " hello world and some extra words",
        StyleMode::Cx,
    );

    for width in [10_u16, 20_u16, 40_u16] {
        let (exact_line_count, _) = item.measure_render_metrics(width, palette);
        let estimated = item.estimate_render_metrics_fast(width, palette, None);

        assert!(
            estimated.content_line_count >= exact_line_count,
            "fast estimate should avoid undercounting wrapped user content: exact={exact_line_count}, estimated={estimated:?}, width={width}"
        );
    }
}

#[test]
fn user_fast_estimate_stays_conservative_for_narrow_wrapped_words() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "abcd efgh ijkl", StyleMode::Cx);
    let width = 11_u16; // Cx 模式下 content_width == 7，容易触发单词边界的额外换行。

    let (exact_line_count, _) = item.measure_render_metrics(width, palette);
    let estimated = item.estimate_render_metrics_fast(width, palette, None);

    assert!(
        estimated.content_line_count >= exact_line_count,
        "fast estimate should avoid undercounting narrow wrapped prose: exact={exact_line_count}, estimated={estimated:?}, width={width}"
    );
}

#[test]
fn user_fast_estimate_stays_conservative_for_wide_grapheme_unbroken_tokens() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "aaaaa中aaa中", StyleMode::Cx);
    let width = 10_u16; // Cx 模式下 content_width == 6，宽字形刚好容易触发“剩 1 列”浪费。

    let (exact_line_count, _) = item.measure_render_metrics(width, palette);
    let estimated = item.estimate_render_metrics_fast(width, palette, None);

    assert!(
        estimated.content_line_count >= exact_line_count,
        "fast estimate should avoid undercounting wide grapheme wrapping: exact={exact_line_count}, estimated={estimated:?}, width={width}"
    );
}

#[test]
fn user_fast_estimate_uses_cjk_breakpoints_in_mixed_prose() {
    let content = "你好，请你随意阅读一下当前目录下的目录和文件情况，不过最多读 10 个文件即可。我只是在测试我的工具，而不是关心你的分析结果内容";
    let exact_line_count = wrap_prompt_visual_lines(content, 102, 0).len();
    let estimated_line_count = estimate_wrapped_line_count_by_display_width(content, 102, 0);

    assert_eq!(exact_line_count, 2);
    assert_eq!(estimated_line_count, exact_line_count);
}

#[test]
fn user_fast_estimate_stays_conservative_for_tabbed_narrow_literal_wraps() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "a\tb", StyleMode::Cx);
    let width = 6_u16; // Cx 模式下 content_width == 2；tab 展开后会跨 continuation line。

    let (exact_line_count, exact_plain_text_len) = item.measure_render_metrics(width, palette);
    let estimated = item.estimate_render_metrics_fast(width, palette, None);

    assert_eq!(exact_line_count, 6);
    assert!(
        estimated.content_line_count >= exact_line_count,
        "fast estimate should avoid undercounting tab-expanded literal wraps: exact={exact_line_count}, estimated={estimated:?}, width={width}"
    );
    assert!(
        estimated.content_char_len >= exact_plain_text_len,
        "fast estimate should avoid shrinking tab-expanded plain text metrics: exact={exact_plain_text_len}, estimated={estimated:?}, width={width}"
    );
}

#[test]
fn user_fast_estimate_stays_conservative_for_short_indent_wide_grapheme_fallback() {
    let palette = default_palette();

    for (style_mode, width) in [(StyleMode::Cx, 6_u16), (StyleMode::Ms, 4_u16)] {
        let item = MessageItem::new_with_style_mode(Sender::User, " 中", style_mode);
        let (exact_line_count, _) = item.measure_render_metrics(width, palette);
        let estimated = item.estimate_render_metrics_fast(width, palette, None);

        assert!(
            estimated.content_line_count >= exact_line_count,
            "fast estimate should preserve the short-indent fallback when a wide grapheme does not fit the first-line remainder: style_mode={style_mode:?}, width={width}, exact={exact_line_count}, estimated={estimated:?}"
        );
    }
}

#[test]
fn prompt_wrap_estimate_reflows_exact_fit_blocks_before_multi_space_followups() {
    let content = "aaa  a  a  a";
    let exact_line_count = wrap_prompt_visual_lines(content, 6, 0).len();
    let estimated_line_count = estimate_wrapped_line_count_by_display_width(content, 6, 0);

    assert_eq!(exact_line_count, 3);
    assert!(
        estimated_line_count >= exact_line_count,
        "fast prose estimate should mirror exact-fit reflow when the next block preserves a multi-space prefix: exact={exact_line_count}, estimated={estimated_line_count}"
    );
}

#[test]
fn hard_wrap_estimate_flushes_full_line_before_zero_width_graphemes() {
    let content = "\t\u{200d}";
    let exact_line_count = wrap_prompt_visual_lines(content, 7, 1).len();
    let estimated_line_count = estimate_wrapped_line_count_by_display_width(content, 7, 1);

    assert_eq!(exact_line_count, 2);
    assert!(
        estimated_line_count >= exact_line_count,
        "literal fast estimate should start a new line before a zero-width grapheme when the current line is already full: exact={exact_line_count}, estimated={estimated_line_count}"
    );
}

#[test]
fn hard_wrap_estimate_treats_zero_width_prefix_as_occupied_content() {
    let content = "\u{200d}中";
    let exact_line_count = wrap_prompt_visual_lines(content, 1, 0).len();
    let estimated_line_count = estimate_hard_wrap_line_count(content, 1, 0);

    assert_eq!(exact_line_count, 2);
    assert!(
        estimated_line_count >= exact_line_count,
        "literal fast estimate should wrap after a zero-width prefix before a wide grapheme: exact={exact_line_count}, estimated={estimated_line_count}"
    );
}

#[test]
fn hard_wrap_visible_text_flushes_a_full_line_before_expanding_a_tab_stop() {
    let (line_count, last_line_width) = estimate_hard_wrap_visible_text("ab\t", 2, 2, 2);

    assert_eq!(line_count, 4);
    assert_eq!(last_line_width, 2);
}

#[test]
fn user_fast_estimate_stays_conservative_when_tab_starts_on_a_full_line() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "ab\t", StyleMode::Cx);
    let width = 6_u16; // Cx 模式下 content_width == 2；tab 应从 continuation line 的列 0 重新计算。

    let (exact_line_count, _) = item.measure_render_metrics(width, palette);
    let estimated = item.estimate_render_metrics_fast(width, palette, None);

    assert_eq!(exact_line_count, 6);
    assert!(
        estimated.content_line_count >= exact_line_count,
        "fast estimate should stay conservative when a tab starts on a full line: exact={exact_line_count}, estimated={estimated:?}, width={width}"
    );
}

#[test]
fn user_fast_estimate_stays_conservative_for_zero_width_prefix_before_wide_grapheme() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "\u{200d}中", StyleMode::Cx);
    let width = 5_u16; // Cx 模式下 content_width == 1；zero-width cluster 也应占住当前逻辑行。

    let (exact_line_count, _) = item.measure_render_metrics(width, palette);
    let estimated = item.estimate_render_metrics_fast(width, palette, None);

    assert_eq!(exact_line_count, 4);
    assert!(
        estimated.content_line_count >= exact_line_count,
        "fast estimate should stay conservative when a zero-width prefix is followed by a wide grapheme: exact={exact_line_count}, estimated={estimated:?}, width={width}"
    );
}

#[test]
fn user_fast_estimate_tracks_plain_text_utf8_bytes_for_prefix_glyphs() {
    let palette = default_palette();
    let width = 20_u16;

    for style_mode in [StyleMode::Cx, StyleMode::Cc] {
        let item = MessageItem::new_with_style_mode(Sender::User, "hello", style_mode);
        let (_, exact_plain_text_len) = item.measure_render_metrics(width, palette);
        let estimated = item.estimate_render_metrics_fast(width, palette, None);

        assert_eq!(
            estimated.content_char_len, exact_plain_text_len,
            "fast estimate should track UTF-8 bytes for user plain text: style_mode={style_mode:?}"
        );
    }
}

#[test]
fn user_fast_estimate_tracks_plain_text_utf8_bytes_for_grapheme_clusters() {
    let palette = default_palette();
    let width = 40_u16;
    let content = format!("👨\u{200d}👩\u{200d}👧\u{200d}👦 {}", "e\u{0301}");

    for style_mode in [StyleMode::Cx, StyleMode::Cc] {
        let item = MessageItem::new_with_style_mode(Sender::User, content.clone(), style_mode);
        let (_, exact_plain_text_len) = item.measure_render_metrics(width, palette);
        let estimated = item.estimate_render_metrics_fast(width, palette, None);

        assert_eq!(
            estimated.content_char_len, exact_plain_text_len,
            "fast estimate should track UTF-8 bytes for grapheme clusters: style_mode={style_mode:?}"
        );
    }
}

#[test]
fn user_fast_estimate_keeps_plain_text_len_conservative_when_visible_line_overflows_width() {
    let palette = default_palette();

    for (style_mode, content, width) in [(StyleMode::Cc, "a", 2_u16), (StyleMode::Cx, "中", 3_u16)]
    {
        let item = MessageItem::new_with_style_mode(Sender::User, content, style_mode);
        let (_, exact_plain_text_len) = item.measure_render_metrics(width, palette);
        let estimated = item.estimate_render_metrics_fast(width, palette, None);

        assert!(
            estimated.content_char_len >= exact_plain_text_len,
            "fast estimate should derive plain-text length from the actual visible line shape instead of assuming every line is capped at width: style_mode={style_mode:?}, width={width}, exact={exact_plain_text_len}, estimated={estimated:?}"
        );
    }
}

#[test]
fn user_fast_estimate_does_not_materialize_wrapped_prompt_lines() {
    let item = MessageItem::new_with_style_mode(
        Sender::User,
        "long user message should stay on the metrics-only path ".repeat(20),
        StyleMode::Cx,
    );

    reset_prompt_text_wrap_call_count();
    let estimated = item.estimate_render_metrics_fast(24, default_palette(), None);

    assert!(estimated.content_line_count > 1);
    assert_eq!(
        prompt_text_wrap_call_count(),
        0,
        "fast estimate should not allocate full wrapped prompt lines"
    );
}

#[test]
fn user_fast_estimate_reports_resize_reuse_and_keeps_plain_text_len_non_decreasing() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(
        Sender::User,
        "keep user resize estimates cheap while preserving selection boundaries",
        StyleMode::Cx,
    );

    let old_width = 20_u16;
    let old_estimate = item.estimate_render_metrics_fast(old_width, palette, None);
    let previous_metrics = TranscriptItemMetrics {
        item_index: 0,
        width: old_width,
        cache_key: item.render_cache_key(),
        content_line_count: old_estimate.content_line_count,
        content_char_len: old_estimate.content_char_len,
        quality: crate::transcript::TranscriptItemMetricsQuality::Estimated,
        is_valid: true,
    };

    let new_estimate = item.estimate_render_metrics_fast(40, palette, Some(previous_metrics));
    assert_eq!(
        new_estimate.source,
        TranscriptEstimateSource::ReusedOnResize
    );
    assert!(new_estimate.content_char_len >= old_estimate.content_char_len);
}

#[test]
fn user_fast_estimate_resize_reuse_stays_conservative_for_narrower_prose_wraps() {
    let palette = default_palette();
    let item = MessageItem::new_with_style_mode(Sender::User, "abc def ghi jkl", StyleMode::Cx);
    let old_width = 11_u16; // Cx 模式下 content_width == 7。
    let new_width = 9_u16; // Cx 模式下 content_width == 5。

    let (old_exact_line_count, old_exact_plain_text_len) =
        item.measure_render_metrics(old_width, palette);
    let previous_metrics = TranscriptItemMetrics {
        item_index: 0,
        width: old_width,
        cache_key: item.render_cache_key(),
        content_line_count: old_exact_line_count,
        content_char_len: old_exact_plain_text_len,
        quality: crate::transcript::TranscriptItemMetricsQuality::Exact,
        is_valid: true,
    };

    let (new_exact_line_count, _) = item.measure_render_metrics(new_width, palette);
    let estimated = item.estimate_render_metrics_fast(new_width, palette, Some(previous_metrics));

    assert_eq!(old_exact_line_count, 4);
    assert_eq!(new_exact_line_count, 6);
    assert!(
        estimated.content_line_count >= new_exact_line_count,
        "resize reuse should stay conservative after narrowing prose wraps: exact={new_exact_line_count}, estimated={estimated:?}, old_exact={old_exact_line_count}"
    );
}

fn plain_line(line: Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}
