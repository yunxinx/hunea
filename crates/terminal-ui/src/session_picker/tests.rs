use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{buffer::Buffer, style::Color};
use runtime_domain::session::{
    RuntimeEvent, SessionPickerRow, SessionPreviewPayload, TranscriptReplayItem,
    TranscriptReplayRole,
};

use crate::runner::TerminalMouseModePreference;
use crate::runtime::RuntimeEventApply;
use crate::test_helpers::{render_model_buffer, rendered_rows};
use crate::{AppEffect, AppEvent, Model, StartupBannerOptions, theme::default_palette};

use super::session_picker_page_size_for_height;

#[test]
fn session_picker_page_size_uses_fullscreen_chrome_with_multi_line_rows() {
    assert_eq!(session_picker_page_size_for_height(12), 2);
    assert_eq!(session_picker_page_size_for_height(20), 4);
    assert_eq!(session_picker_page_size_for_height(7), 1);
}

#[test]
fn session_picker_filters_and_resumes_selected_session() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
    ]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('b'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('e'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::ResumeSession {
            session_id: "session-b".to_string(),
        })
    );
    assert!(!model.session_picker_active());
}

#[test]
fn session_picker_space_requests_preview_for_selected_session() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
    ]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_eq!(
        effect,
        Some(AppEffect::OpenSessionPreview {
            session_id: "session-b".to_string(),
        })
    );
    assert!(
        model.session_picker_active(),
        "preview request should keep the picker open behind the preview overlay"
    );
}

#[test]
fn session_picker_search_mode_treats_space_as_query_text() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_eq!(effect, None);
    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open while search is active");
    assert_eq!(
        state.search_query(),
        " ",
        "space should be typed into the search query instead of opening preview"
    );
    assert!(!model.session_preview_active());
}

#[test]
fn session_picker_empty_list_keeps_shared_left_inset() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(Vec::new());

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));

    assert!(
        rows[2].starts_with("  No sessions"),
        "empty-list copy should align with the shared two-space inset: {rows:?}"
    );
}

#[test]
fn session_preview_opens_on_latest_page_and_space_returns_to_picker() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);

    model.apply_runtime_event(RuntimeEvent::SessionPreviewLoaded {
        payload: SessionPreviewPayload {
            session_id: "session-a".to_string(),
            transcript: (0..12)
                .map(|index| TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::Assistant,
                    content: format!("preview answer {index}"),
                })
                .collect(),
        },
    });

    assert!(model.session_preview_active());
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows.iter().any(|row| row.contains("preview answer 11")),
        "preview should open at the latest page: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains(" Page ") && row.contains('/')),
        "preview should use a page rule instead of a percentage rule: {rows:?}"
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_eq!(effect, None);
    assert!(!model.session_preview_active());
    assert!(
        model.session_picker_active(),
        "space should return from preview to the picker"
    );
}

#[test]
fn session_preview_footer_names_vertical_and_horizontal_page_keys() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.apply_runtime_event(preview_loaded_event("session-a", 12));

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    let footer = rows.last().expect("preview should render a footer row");

    assert!(
        footer.starts_with("  "),
        "preview footer should keep the shared two-space left inset: {rows:?}"
    );
    assert!(
        footer.contains("↑/←/h") && footer.contains("↓/→/l"),
        "preview footer should name both vertical and horizontal page keys: {rows:?}"
    );
}

#[test]
fn session_preview_ignores_q_as_back_key() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.apply_runtime_event(preview_loaded_event("session-a", 12));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))));

    assert_eq!(effect, None);
    assert!(
        model.session_preview_active(),
        "q should not return from preview; Esc and Space remain the explicit back keys"
    );
}

#[test]
fn session_preview_enter_resumes_preview_session() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.apply_runtime_event(RuntimeEvent::SessionPreviewLoaded {
        payload: SessionPreviewPayload {
            session_id: "session-a".to_string(),
            transcript: vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: "preview answer".to_string(),
            }],
        },
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::ResumeSession {
            session_id: "session-a".to_string(),
        })
    );
    assert!(!model.session_preview_active());
    assert!(!model.session_picker_active());
}

#[test]
fn session_preview_uses_alternate_scroll_mouse_mode_over_picker() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);

    model.apply_runtime_event(preview_loaded_event("session-a", 12));

    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::NativeWithAlternateScroll,
        "preview should use the same alternate-scroll terminal mode as ctrl+t even while the picker is still open behind it"
    );
}

#[test]
fn session_preview_maps_alternate_scroll_arrows_to_page_navigation() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.apply_runtime_event(preview_loaded_event("session-a", 12));

    let latest_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        latest_page_rows
            .iter()
            .any(|row| row.contains(" Page 4/4 ")),
        "preview should start on the latest page: {latest_page_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        previous_page_rows
            .iter()
            .any(|row| row.contains(" Page 3/4 ")),
        "terminal scroll-up arrow should page left: {previous_page_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    let next_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        next_page_rows.iter().any(|row| row.contains(" Page 4/4 ")),
        "terminal scroll-down arrow should page right: {next_page_rows:?}"
    );
}

#[test]
fn session_preview_maps_mouse_wheel_to_page_navigation_if_delivered() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.apply_runtime_event(preview_loaded_event("session-a", 12));

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        previous_page_rows
            .iter()
            .any(|row| row.contains(" Page 3/4 ")),
        "mouse wheel up should page left if a wheel event is delivered: {previous_page_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let next_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        next_page_rows.iter().any(|row| row.contains(" Page 4/4 ")),
        "mouse wheel down should page right if a wheel event is delivered: {next_page_rows:?}"
    );
}

#[test]
fn session_picker_uses_alternate_scroll_mouse_mode() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..3).map(numbered_picker_row).collect());

    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::NativeWithAlternateScroll,
        "resume picker should let the terminal own selection while mapping wheel bursts to Up/Down"
    );
}

#[test]
fn session_picker_enables_page_scroll_burst_coalescing() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..3).map(numbered_picker_row).collect());

    assert!(
        model
            .terminal_input_coalescing()
            .has_page_scroll_burst_coalescing,
        "resume picker should coalesce high-frequency wheel bursts like preview"
    );
}

#[test]
fn session_picker_maps_mouse_wheel_to_vertical_navigation_if_delivered() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..6).map(numbered_picker_row).collect());

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let down_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        down_rows[0].starts_with("  Resume Session (2 of 6)"),
        "mouse wheel down should move selection down by one row: {down_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let up_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        up_rows[0].starts_with("  Resume Session (1 of 6)"),
        "mouse wheel up should move selection up by one row: {up_rows:?}"
    );
}

#[test]
fn session_picker_esc_closes_without_effect() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(!model.session_picker_active());
}

#[test]
fn session_picker_esc_closes_while_search_is_active() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('a'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(model.session_picker_active());
    assert!(
        !model
            .session_picker
            .as_ref()
            .expect("picker should stay open after leaving search")
            .is_searching(),
        "Esc should leave search mode before closing the picker"
    );
    assert!(
        model
            .session_picker
            .as_ref()
            .expect("picker should stay open after leaving search")
            .search_query()
            .is_empty(),
        "Esc should clear the active search filter"
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(!model.session_picker_active());
}

#[test]
fn session_picker_empty_search_mode_requires_esc_before_close() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![picker_row(
        "session-a",
        "alpha work",
        "first alpha",
        "answer alpha",
        "/tmp/alpha",
    )]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    let searched_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        searched_rows[0].starts_with("  Resume Session (1 of 1) · Search: "),
        "empty active search should still be shown in the header: {searched_rows:?}"
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(
        model.session_picker_active(),
        "Esc should only leave empty search mode before closing the picker"
    );
    let rows_after_search_exit = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        !rows_after_search_exit[0].contains("Search:"),
        "Esc should remove the empty search header before a second Esc closes: {rows_after_search_exit:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.session_picker_active());
}

#[test]
fn session_picker_keeps_selection_when_empty_search_exits() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..5).map(numbered_picker_row).collect());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert!(model.session_picker_active());
    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open after leaving search");
    assert!(!state.is_searching());
    assert_eq!(
        state.selected_visible_position(),
        Some(2),
        "leaving empty search should preserve the previously selected session"
    );
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        rows[0].starts_with("  Resume Session (3 of 5)"),
        "header should keep the third selected session after leaving search: {rows:?}"
    );
}

#[test]
fn session_picker_restores_selection_by_session_id_after_row_refresh() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
        picker_row(
            "session-c",
            "gamma work",
            "first gamma",
            "answer gamma",
            "/tmp/gamma",
        ),
    ]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-c",
            "gamma work",
            "first gamma",
            "answer gamma",
            "/tmp/gamma",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
    ]);

    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open after row refresh");
    assert_eq!(
        state.selected_row().map(|row| row.session_id.as_str()),
        Some("session-b"),
        "row refresh should preserve the selected session id, not the old list index"
    );
}

#[test]
fn session_picker_clearing_query_keeps_search_mode_active() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
    ]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('b'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));

    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open after clearing search query");
    assert!(
        state.is_searching(),
        "Backspace should clear text without leaving search mode"
    );
    assert!(
        state.search_query().is_empty(),
        "Backspace should remove the final search character"
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        rows[0].starts_with("  Resume Session (2 of 2) · Search: "),
        "empty query should remain visibly in search mode until Esc: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("first alpha"))
            && rows.iter().any(|row| row.contains("first beta")),
        "empty query should restore the full filtered list while search mode stays active: {rows:?}"
    );
}

#[test]
fn session_picker_search_mode_treats_hjkl_as_query_text() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..6).map(numbered_picker_row).collect());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('h'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('j'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('k'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('l'))));

    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open while search is active");
    assert_eq!(
        state.search_query(),
        "hjkl",
        "hjkl should be typed into search instead of driving list navigation"
    );
    assert_eq!(
        state.filtered_count(),
        0,
        "hjkl query should produce no visible matches"
    );

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));
    assert!(
        rows[0].starts_with("  Resume Session (0 of 0) · Search: hjkl"),
        "search header should show hjkl query text: {rows:?}"
    );
}

#[test]
fn session_picker_renders_fixed_header_rule_footer_and_page_label() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..6).map(numbered_picker_row).collect());

    let initial_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        initial_rows[0].starts_with("  Resume Session (1 of 6)"),
        "header should show selected position over filtered count: {initial_rows:?}"
    );
    assert!(
        !initial_rows[0].contains("Search:"),
        "empty search should not be shown in the fixed header: {initial_rows:?}"
    );
    assert!(
        initial_rows[1]
            .trim()
            .chars()
            .all(|character| character == '╌'),
        "header/list separator should use the same dashed rule as the edit preview: {initial_rows:?}"
    );
    assert!(
        initial_rows[5].trim().is_empty() && initial_rows[6].contains("first 1"),
        "session rows should keep one blank line between items: {initial_rows:?}"
    );
    assert!(
        initial_rows[10].contains(" Page 1/3 "),
        "rule should carry right-aligned page state: {initial_rows:?}"
    );
    assert!(
        initial_rows[11].starts_with("  "),
        "footer should keep the shared two-space left inset: {initial_rows:?}"
    );
    assert!(
        initial_rows[11].contains("Esc close")
            && initial_rows[11].contains("Type / to search")
            && initial_rows[11].contains("h/l page"),
        "footer should stay fixed at the bottom with search and page hints: {initial_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('5'))));
    let searched_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        searched_rows[0].starts_with("  Resume Session (1 of 1) · Search: 5"),
        "active search should be appended to the fixed header: {searched_rows:?}"
    );
    assert!(
        searched_rows.iter().any(|row| row.contains("first 5")),
        "filtered list should keep matching row visible: {searched_rows:?}"
    );
    assert!(
        searched_rows.iter().all(|row| !row.contains("first 4")),
        "filtered list should hide non-matching rows: {searched_rows:?}"
    );
}

#[test]
fn session_picker_uses_prompt_block_and_search_label_color() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..6).map(numbered_picker_row).collect());

    let initial_buffer = render_model_buffer(&mut model, 60, 12);
    let initial_rows = rendered_rows(&initial_buffer);
    assert!(
        !initial_rows.iter().any(|row| row.contains('➜')),
        "selected session should no longer use the arrow marker: {initial_rows:?}"
    );
    assert_selected_session_prompt_block(&initial_buffer, 2);
    assert_text_cells_use_color(&initial_buffer, "first 0", default_palette().main);
    assert_text_cells_use_color(&initial_buffer, "first 1", default_palette().secondary);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('5'))));

    let search_buffer = render_model_buffer(&mut model, 60, 12);
    assert_text_cells_use_color(&search_buffer, "Search:", default_palette().command_accent);
    assert_text_cells_use_color(&search_buffer, "5", default_palette().main);
}

#[test]
fn session_picker_pages_with_left_right_and_jk_navigation() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows((0..6).map(numbered_picker_row).collect());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('l'))));
    let second_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        second_page_rows[0].starts_with("  Resume Session (3 of 6)"),
        "l should move to the next page and select its first row: {second_page_rows:?}"
    );
    assert!(
        second_page_rows[10].contains(" Page 2/3 "),
        "page label should follow horizontal navigation: {second_page_rows:?}"
    );
    assert!(
        second_page_rows.iter().any(|row| row.contains("first 2"))
            && second_page_rows.iter().all(|row| !row.contains("first 0")),
        "second page should render page rows, not a scroll window from row zero: {second_page_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('k'))));
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        previous_page_rows[0].starts_with("  Resume Session (2 of 6)"),
        "k at the top of a page should move to the previous row on the previous page: {previous_page_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    let right_key_rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        right_key_rows[10].contains(" Page 2/3 "),
        "Right should page forward like l: {right_key_rows:?}"
    );
}

#[test]
fn session_picker_allows_terminal_mouse_selection() {
    let mut model = ready_model();

    assert!(model.wants_mouse_capture());
    model.open_session_picker_loading();

    assert!(
        !model.wants_mouse_capture(),
        "resume picker should release mouse capture like ctrl+t overlay"
    );
}

#[test]
fn session_picker_meta_uses_relative_age_directory_and_size() {
    let row = SessionPickerRow {
        updated_at_ms: 1_000_000,
        work_dir: "/tmp/project".to_string(),
        size_bytes: Some(1536),
        ..picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/project",
        )
    };

    assert_eq!(
        super::session_picker_meta_text_at(&row, 1_000_000 + 2 * 60 * 60 * 1000),
        "2h · /tmp/project · 1.5 KiB"
    );
    assert_eq!(
        super::session_picker_meta_text_at(&row, 1_000_000 + 3 * 24 * 60 * 60 * 1000),
        "3d · /tmp/project · 1.5 KiB"
    );
    assert_eq!(
        super::session_picker_meta_text_at(
            &row,
            1_000_000 + ((3 * 24 * 60) + (5 * 60) + 6) * 60 * 1000
        ),
        "3d 5h 6m · /tmp/project · 1.5 KiB"
    );
}

#[test]
fn session_picker_meta_uses_picker_open_time_as_relative_age_reference() {
    let mut model = ready_model();
    model.open_session_picker_loading_at(1_000_000 + 5 * 60 * 1000);
    model.apply_session_picker_rows(vec![SessionPickerRow {
        updated_at_ms: 1_000_000,
        work_dir: "/tmp/project".to_string(),
        size_bytes: Some(1536),
        ..picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/project",
        )
    }]);

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 12));

    assert!(
        rows.iter()
            .any(|row| row.contains("5m · /tmp/project · 1.5 KiB")),
        "relative age should use the picker opening time, not the current render time: {rows:?}"
    );
}

#[test]
fn session_picker_exposes_render_state_without_leaking_list_internals() {
    let mut model = ready_model();
    model.open_session_picker_loading();
    model.apply_session_picker_rows(vec![
        picker_row(
            "session-a",
            "alpha work",
            "first alpha",
            "answer alpha",
            "/tmp/alpha",
        ),
        picker_row(
            "session-b",
            "beta work",
            "first beta",
            "answer beta",
            "/tmp/beta",
        ),
    ]);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('b'))));

    let state = model
        .session_picker
        .as_ref()
        .expect("picker should stay open while filtering");
    assert_eq!(state.filtered_count(), 1);
    assert!(state.has_rows());
    assert!(state.has_filtered_rows());
    assert_eq!(state.selected_visible_position(), Some(0));
    assert!(state.is_selected_visible_position(0));
    assert!(!state.is_selected_visible_position(1));
}

fn assert_text_cells_use_color(buffer: &Buffer, text: &str, expected: Color) {
    let (row, column) = text_position(buffer, text).expect("text should be rendered");
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].fg,
            expected,
            "{text:?} should use the expected foreground color at offset {offset}"
        );
    }
}

fn assert_selected_session_prompt_block(buffer: &Buffer, first_row: u16) {
    for row in first_row..first_row + 3 {
        assert_eq!(
            buffer[(0, row)].symbol(),
            "█",
            "selected session row {row} should render a one-cell prompt block"
        );
        assert_eq!(
            buffer[(0, row)].fg,
            default_palette().command_accent,
            "selected session prompt block should use command accent color"
        );
        assert_eq!(
            buffer[(1, row)].symbol(),
            " ",
            "selected session should keep one column of spacing after the prompt block"
        );
        assert_eq!(
            buffer[(1, row)].fg,
            Color::Reset,
            "only the one-cell prompt block should use the accent color"
        );
    }
}

fn text_position(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        if let Some(byte_index) = rendered.find(needle) {
            let column = rendered[..byte_index].chars().count() as u16;
            return Some((row, column));
        }
    }

    None
}

fn ready_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 12);
    model.set_palette(default_palette(), true);
    model
}

fn picker_row(
    session_id: &str,
    title: &str,
    first_user_message: &str,
    last_assistant_message: &str,
    work_dir: &str,
) -> SessionPickerRow {
    SessionPickerRow {
        session_id: session_id.to_string(),
        title: title.to_string(),
        first_user_message: first_user_message.to_string(),
        last_assistant_message: last_assistant_message.to_string(),
        updated_at_ms: 0,
        work_dir: work_dir.to_string(),
        size_bytes: Some(2048),
        model: Some("qwen3".to_string()),
    }
}

fn numbered_picker_row(index: usize) -> SessionPickerRow {
    picker_row(
        &format!("session-{index}"),
        &format!("title {index}"),
        &format!("first {index}"),
        &format!("answer {index}"),
        &format!("/tmp/project-{index}"),
    )
}

fn preview_loaded_event(session_id: &str, message_count: usize) -> RuntimeEvent {
    RuntimeEvent::SessionPreviewLoaded {
        payload: SessionPreviewPayload {
            session_id: session_id.to_string(),
            transcript: (0..message_count)
                .map(|index| TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::Assistant,
                    content: format!("preview answer {index}"),
                })
                .collect(),
        },
    }
}
