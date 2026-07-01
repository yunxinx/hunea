use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use runtime_domain::prompt_assembly::PromptSourceOrigin;

use super::Composer;
use crate::{StyleMode, theme::default_palette};

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
fn ctrl_u_deletes_current_line_before_cursor_only() {
    let mut composer = composer_with_cursor(test_composer(80, 4, "alpha\nbeta gamma"), 10);

    composer.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

    assert_eq!(composer.value(), "alpha\n gamma");
    assert_eq!(composer.cursor_position(), (1, 0));
}

#[test]
fn repeated_ctrl_u_moves_to_previous_line_end() {
    let mut composer = composer_with_cursor(test_composer(80, 4, "alpha\nbeta gamma"), 10);

    composer.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    composer.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));

    assert_eq!(composer.value(), "alpha gamma");
    assert_eq!(composer.cursor_position(), (0, 5));
}

#[test]
fn alt_word_keys_move_by_word_boundaries() {
    let mut composer = test_composer(80, 4, "alpha beta gamma");

    composer.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(composer.cursor_position(), (0, 11));

    composer.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
    assert_eq!(composer.cursor_position(), (0, 6));

    composer.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::ALT));
    assert_eq!(composer.cursor_position(), (0, 10));
}

#[test]
fn word_delete_updates_yank_buffer() {
    let mut composer = test_composer(80, 4, "alpha beta gamma");

    composer.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha beta ");

    composer.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha beta gamma");
}

#[test]
fn forward_word_delete_updates_yank_buffer() {
    let mut composer = composer_with_cursor(test_composer(80, 4, "alpha beta gamma"), 6);

    composer.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::ALT));
    assert_eq!(composer.value(), "alpha  gamma");

    composer.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha beta gamma");
}

#[test]
fn ctrl_k_kills_to_line_end_and_yanks() {
    let mut composer = composer_with_cursor(test_composer(80, 4, "alpha beta\ngamma"), 5);

    composer.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha\ngamma");

    composer.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha beta\ngamma");
}

#[test]
fn ctrl_u_kill_can_be_yanked() {
    let mut composer = composer_with_cursor(test_composer(80, 4, "alpha\nbeta gamma"), 10);

    composer.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha\n gamma");

    composer.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha\nbeta gamma");
}

#[test]
fn ctrl_z_undoes_composer_edits_without_changing_yank_buffer() {
    let mut composer = test_composer(80, 4, "alpha beta");

    composer.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha ");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha beta");

    composer.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "alpha betabeta");
}

#[test]
fn undo_history_keeps_configured_number_of_snapshots() {
    let mut composer = Composer::new_with_undo_limit(StyleMode::Ms, 2);

    for text in ["a", "b", "c"] {
        composer.insert_text(text);
    }

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "ab");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "a");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "a");
}

#[test]
fn ctrl_z_does_not_restore_partial_emoji_grapheme() {
    let mut composer = Composer::new(StyleMode::Ms);

    for character in "🈶️".chars() {
        composer.handle_key(KeyEvent::from(KeyCode::Char(character)));
    }
    assert_eq!(composer.value(), "🈶️");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));

    assert_eq!(composer.value(), "");
}

#[test]
fn ctrl_z_does_not_merge_separate_cjk_graphemes_without_ime_boundary() {
    let mut composer = Composer::new(StyleMode::Ms);

    for character in "你好，今天怎么样".chars() {
        composer.handle_key(KeyEvent::from(KeyCode::Char(character)));
    }
    assert_eq!(composer.value(), "你好，今天怎么样");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "你好，今天怎么");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "你好，今天怎");
}

#[test]
fn ctrl_z_keeps_unicode_variation_selector_with_base_emoji() {
    let mut composer = Composer::new(StyleMode::Ms);

    for character in "☃️".chars() {
        composer.handle_key(KeyEvent::from(KeyCode::Char(character)));
    }
    assert_eq!(composer.value(), "☃️");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "");
}

#[test]
fn cursor_movement_starts_a_new_plain_input_undo_step() {
    let mut composer = Composer::new(StyleMode::Ms);

    for character in "ab".chars() {
        composer.handle_key(KeyEvent::from(KeyCode::Char(character)));
    }
    composer.handle_key(KeyEvent::from(KeyCode::Left));
    composer.handle_key(KeyEvent::from(KeyCode::Char('X')));
    assert_eq!(composer.value(), "aXb");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "ab");

    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));
    assert_eq!(composer.value(), "a");
}

#[test]
fn undoable_replace_restores_previous_draft() {
    let mut composer = test_composer(80, 4, "draft before edit");

    composer.replace_text_and_move_to_end_for_edit("draft after edit");
    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));

    assert_eq!(composer.value(), "draft before edit");
}

#[test]
fn reset_replace_clears_previous_undo_history() {
    let mut composer = test_composer(80, 4, "draft");
    composer.handle_key(KeyEvent::from(KeyCode::Char('!')));

    composer.reset_text_and_move_to_end("prefilled from transcript");
    composer.handle_key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::CONTROL));

    assert_eq!(composer.value(), "prefilled from transcript");
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
fn content_width_reserves_two_cell_right_padding() {
    let mut composer = Composer::new(StyleMode::Ms);
    composer.set_width(12);

    assert_eq!(composer.content_width(), 8);
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

#[test]
fn skill_binding_survives_edit_outside_bound_token() {
    let mut composer = test_composer(80, 3, "$code");
    assert!(composer.replace_current_skill_token(
        "code-review",
        "/tmp/code-review/SKILL.md",
        PromptSourceOrigin::Project,
    ));

    composer.insert_text(" please inspect this");

    let source_message = composer.source_message();
    assert_eq!(source_message.skill_bindings().len(), 1);
    assert_eq!(source_message.skill_bindings()[0].skill_name, "code-review");
}

#[test]
fn skill_binding_drops_immediately_after_manual_token_edit() {
    let mut composer = test_composer(80, 3, "$code");
    assert!(composer.replace_current_skill_token(
        "code-review",
        "/tmp/code-review/SKILL.md",
        PromptSourceOrigin::Project,
    ));

    composer.handle_key(KeyEvent::from(KeyCode::Left));
    composer.handle_key(KeyEvent::from(KeyCode::Left));
    composer.handle_key(KeyEvent::from(KeyCode::Char('x')));

    let source_message = composer.source_message();
    assert!(source_message.skill_bindings().is_empty());
}

#[test]
fn bound_skill_token_renders_with_command_accent_before_submit() {
    let mut composer = test_composer(80, 3, "$code");
    assert!(composer.replace_current_skill_token(
        "code-review",
        "/tmp/code-review/SKILL.md",
        PromptSourceOrigin::Project,
    ));

    let palette = default_palette();
    let document = composer.render_document(palette);
    let skill_span = document
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "$code-review")
        .expect("bound skill token should render as a distinct span");

    assert_eq!(skill_span.style.fg, Some(palette.command_accent));
    assert_ne!(skill_span.style.fg, Some(Color::Reset));
}

#[test]
fn bound_skill_token_keeps_same_background_as_live_cx_input() {
    let mut composer = Composer::new(StyleMode::Cx);
    composer.set_width(82);
    composer.set_height(3);
    composer.set_text_for_test("$code");
    assert!(composer.replace_current_skill_token(
        "code-review",
        "/tmp/code-review/SKILL.md",
        PromptSourceOrigin::Project,
    ));

    let palette = default_palette();
    let document = composer.render_document(palette);
    let skill_span = document
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "$code-review")
        .expect("bound skill token should render as a distinct span");
    let plain_text_span = document
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == " ")
        .expect("composer should render trailing plain text/fill span");

    assert_eq!(skill_span.style.fg, Some(palette.command_accent));
    assert_eq!(
        skill_span.style.bg, plain_text_span.style.bg,
        "bound skill token should only change foreground color, not carve out a different background"
    );
}

fn test_composer(width: u16, height: u16, value: &str) -> Composer {
    let mut composer = Composer::new(StyleMode::Ms);
    // 测试里的 width 表达可编辑内容加 prompt 的旧视觉宽度；真实 frame 还要包含右侧留白。
    composer.set_width(width.saturating_add(2));
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
    composer.reset_text_and_move_to_end("中英 mixed long composer text ".repeat(120));
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
