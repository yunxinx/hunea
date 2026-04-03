use crossterm::event::{KeyCode, KeyEvent};

use super::Composer;
use crate::frontend::tui::theme::default_palette;

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
fn render_wraps_chinese_text_at_the_line_edge() {
    let composer = test_composer(6, 2, "中文中");

    let render = composer.render(default_palette());

    assert_eq!(plain_lines(&render), vec!["┃ 中文", "  中"]);
    assert_eq!(render.cursor_y, 1);
    assert_eq!(render.cursor_x, 4);
}

fn test_composer(width: u16, height: u16, value: &str) -> Composer {
    let mut composer = Composer::default();
    composer.set_width(width);
    composer.set_height(height);
    composer.set_text_for_test(value);
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
