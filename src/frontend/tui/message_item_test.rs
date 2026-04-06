use super::*;
use crate::frontend::tui::{
    selection::SelectableLineRange,
    theme::{default_palette, secondary_text_style, surface_text_style},
    transcript::{CachedLineAnchors, CachedRenderBlock},
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
        .render_lines(20, default_palette())
        .into_iter()
        .map(plain_line)
        .collect::<Vec<_>>();

    assert_eq!(lines, vec!["Overview of the API"]);
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
    assert_eq!(ranges[1], SelectableLineRange::blank_anchor(0, 20));
    assert_eq!(ranges[2], SelectableLineRange::new(2, 5));
    assert_eq!(ranges[3], SelectableLineRange::default());
}

#[test]
fn user_message_selectable_ranges_ignore_trailing_fill() {
    let item = MessageItem::new(Sender::User, "hi");

    let ranges = item.render_selectable_line_ranges(10, default_palette());

    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[1], SelectableLineRange::new(0, 4));
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
        lines: Rc::new(Vec::new()),
        projected_user: Some(Rc::new(projection)),
        line_count: eager_line_count,
        plain_line_byte_lens: Rc::new(Vec::new()),
        anchors: CachedLineAnchors::default(),
        plain_text_char_len: 0,
    };
    let eager_block = CachedRenderBlock {
        cache_key: 0,
        width,
        lines: Rc::new(eager_lines),
        projected_user: None,
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

fn plain_line(line: Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>()
}
