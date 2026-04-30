use crossterm::event::{KeyCode, KeyEvent};

use super::Composer;
use crate::frontend::tui::{StyleMode, theme::default_palette};

#[test]
fn right_moves_over_the_full_combining_cluster() {
    let mut composer = test_composer(12, 4, "e\u{301}");
    composer.move_to_begin_for_test();

    composer.handle_key(KeyEvent::from(KeyCode::Right));

    assert_eq!(composer.cursor_position(), (0, "e\u{301}".chars().count()));
}

#[test]
fn delete_at_line_end_merges_the_next_line() {
    let mut composer = test_composer(12, 4, "ab\ncd");
    composer.move_to_begin_for_test();
    composer.handle_key(KeyEvent::from(KeyCode::End));

    composer.handle_key(KeyEvent::from(KeyCode::Delete));

    assert_eq!(composer.value(), "abcd");
    assert_eq!(composer.cursor_position(), (0, 2));
}

#[test]
fn down_moves_across_empty_lines_in_custom_visual_coordinates() {
    let mut composer = test_composer(12, 4, "ab\n\ncd");
    composer.move_to_begin_for_test();
    composer.handle_key(KeyEvent::from(KeyCode::Right));

    composer.handle_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(composer.cursor_position(), (1, 0));

    composer.handle_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(composer.cursor_position(), (2, 0));
}

#[test]
fn page_down_uses_custom_visual_lines_for_zwj_sequences() {
    let mut composer = test_composer(3, 1, "👨‍👩‍👧👨‍👩‍👦👨‍👩‍👧‍👦");
    composer.move_to_begin_for_test();

    composer.handle_key(KeyEvent::from(KeyCode::PageDown));

    let render = composer.render(default_palette());
    assert_eq!(plain_lines(&render), vec!["  👨‍👩‍👦"]);
    assert_eq!(render.cursor_y, 0);
}

#[test]
fn home_moves_to_the_current_line_start() {
    let mut composer = test_composer(8, 2, "ab\ncd");
    composer.handle_key(KeyEvent::from(KeyCode::Left));

    composer.handle_key(KeyEvent::from(KeyCode::Home));

    assert_eq!(composer.cursor_position(), (1, 0));
}

#[test]
fn end_moves_to_the_current_line_end() {
    let mut composer = test_composer(8, 2, "ab\ncde");
    composer.move_to_begin_for_test();
    composer.handle_key(KeyEvent::from(KeyCode::Right));

    composer.handle_key(KeyEvent::from(KeyCode::End));

    assert_eq!(composer.cursor_position(), (0, 2));
}

#[test]
fn render_wraps_chinese_text_at_the_line_edge() {
    let composer = test_composer(6, 2, "中文中");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ 中文", "  中"]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 4);
}

#[test]
fn render_shows_editable_boundary_space_on_continuation_line() {
    let composer = test_composer(7, 2, "hello ");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ hello", "   "]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 3);
}

#[test]
fn render_maps_hidden_boundary_space_to_continuation_start() {
    let composer = test_composer(7, 2, "hello world");

    let render = composer_with_cursor(composer, 6).render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ hello", "  world"]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 2);
}

#[test]
fn render_reflows_boundary_word_with_trailing_space() {
    let composer = test_composer(22, 2, &(String::from("aaaaaaaaaaaaaaaaaa b ")));

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ aaaaaaaaaaaaaaaaaa", "  b "]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 4);
}

#[test]
fn render_keeps_later_text_on_wrapped_boundary_line() {
    let composer = test_composer(22, 2, &(String::from("aaaaaaaaaaaaaaaaaa b   c")));

    let render = composer.render(default_palette());

    assert_eq!(
        plain_lines(&render),
        vec!["┃ aaaaaaaaaaaaaaaaaa", "  b   c"]
    );
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 7);
}

#[test]
fn render_preserves_long_leading_spaces_on_wrapped_continuation_line() {
    let composer = test_composer(7, 2, "abc d    e");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ abc d", "      e"]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 7);
}

#[test]
fn render_keeps_short_indented_cursor_on_first_visual_line() {
    let composer = composer_with_cursor(test_composer(7, 2, " abc def"), 4);

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃  abc", "  def"]);
    assert_eq!(render.cursor_y, 0);
    assert_eq!(render.cursor_x, 6);
}

#[test]
fn render_keeps_short_indented_hard_wrapped_cursor_at_line_end() {
    let composer = test_composer(4, 3, " abcde");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃  a", "  bc", "  de"]);
    assert_eq!(render.cursor_y, 2);
    assert_eq!(render.cursor_x, 4);
}

#[test]
fn render_wraps_short_indented_wide_glyph_without_overwide_line() {
    let composer = test_composer(4, 2, " 中");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃  ", "  中"]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 4);
}

#[test]
fn render_expands_tabs_using_prompt_aware_stops() {
    let composer = test_composer(10, 1, "a\tb");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ a     b"]);
    assert_eq!(render.cursor_y, 0);
}

#[test]
fn current_at_token_only_uses_whitespace_delimited_mentions() {
    let cases = [
        ("@", 1, Some("")),
        ("@src/main.rs", 5, Some("src/main.rs")),
        ("open @src/main.rs now", 9, Some("src/main.rs")),
        ("open @ now", 6, Some("")),
        ("open @ now", 7, None),
        ("email test@example.com", 18, None),
        ("pkg @scope/name@latest", 19, Some("scope/name@latest")),
    ];

    for (value, cursor, expected) in cases {
        let composer = composer_with_cursor(test_composer(80, 3, value), cursor);

        assert_eq!(
            composer.current_at_token().as_deref(),
            expected,
            "value={value:?} cursor={cursor}"
        );
    }
}

#[test]
fn replace_current_at_token_keeps_surrounding_text_and_moves_cursor() {
    let mut composer = test_composer(80, 3, "open @sr");

    assert!(composer.replace_current_at_token("@src/main.rs "));

    assert_eq!(composer.value(), "open @src/main.rs ");
    assert_eq!(
        composer.cursor_position(),
        (0, "open @src/main.rs ".chars().count())
    );
}

fn test_composer(width: u16, height: u16, value: &str) -> Composer {
    let mut composer = Composer::new(StyleMode::Ms);
    composer.set_width(width);
    composer.set_height(height);
    composer.set_text_for_test(value);
    composer
}

fn composer_with_cursor(mut composer: Composer, cursor: usize) -> Composer {
    let current = composer.value().chars().count();
    for _ in 0..current.saturating_sub(cursor) {
        composer.handle_key(KeyEvent::from(KeyCode::Left));
    }

    composer
}

fn plain_lines(render: &super::render::RenderResult) -> Vec<String> {
    render
        .lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn cursor_position_for_line_anchor_click_does_not_rewrap_long_composer() {
    use super::{
        cursor_position_for_line_anchor_click, reset_visual_lines_call_count,
        visual_lines_call_count,
    };

    let mut composer = Composer::new(StyleMode::Cx);
    composer.set_width(80);
    composer.replace_text_and_move_to_end("中英 mixed long composer text ".repeat(120));
    let document = composer.render_document(default_palette());
    let anchor = document.anchors[0];

    reset_visual_lines_call_count();
    let position = cursor_position_for_line_anchor_click(&composer, anchor, 10);

    assert!(position.is_some());
    assert_eq!(
        visual_lines_call_count(),
        0,
        "click hit-testing should use the clicked anchor segment instead of wrapping the full composer"
    );
}
