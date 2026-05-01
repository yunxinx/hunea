use std::rc::Rc;

use super::*;
use crate::frontend::tui::{
    AppEffect, AppEvent, Sender, StyleMode, document::DocumentAnchorRegion,
};
use crate::runtime::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use crate::runtime::native::ProviderKind;
use crate::runtime::phrases::StatusPhraseOrder;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{Terminal, backend::TestBackend};
use std::path::{Path, PathBuf};

fn progressive_exactization_fixture() -> Model {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..40 {
        let content = match index % 4 {
            0 => {
                format!("# Overview {index} alpha beta gamma delta epsilon zeta eta theta iota")
            }
            1 => format!(
                "```rust\nfn helper_{index}() {{ println!(\"alpha beta gamma delta epsilon zeta eta theta iota\"); }}\n```"
            ),
            2 => format!(
                "| key | value |\n| --- | --- |\n| alpha beta gamma {index} | delta epsilon zeta eta theta |\n| iota kappa lambda | mu nu xi omicron pi |"
            ),
            _ => format!(
                "__init__ item {index} keeps markdown emphasis and heading-like text wrapped across the viewport"
            ),
        };
        model
            .transcript_mut()
            .append_message(Sender::Assistant, content);
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();
    model
}

fn idle_refinement_fixture() -> Model {
    let mut model = progressive_exactization_fixture();
    model
        .composer_mut()
        .set_text_for_test("draft line one\ndraft line two\ndraft line three");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model
}

fn apply_scrolled_offset(model: &mut Model, offset: usize, manual_scroll: bool) {
    let layout = model.build_document_layout();
    let composer_offset = model.current_composer_viewport_offset(&layout, offset);
    model.apply_document_viewport_position(&layout, offset, composer_offset, false, manual_scroll);
}

#[test]
fn overflowing_document_bottom_slice_keeps_full_draft_height() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("1\n2\n3");
    model.sync_composer_height();
    model.sync_document_viewport_to_bottom();

    let layout = model.build_document_layout();
    assert_eq!(layout.composer_line_count, 3);

    let viewport = model.build_document_viewport(&layout);
    let rendered = viewport.plain_lines.clone();
    assert_eq!(
        rendered,
        vec![
            String::new(),
            "┃ 1".to_string(),
            "┃ 2".to_string(),
            "┃ 3".to_string(),
        ]
    );
}

#[test]
fn transcript_plain_items_use_assistant_markdown_render_path() {
    let mut model = Model::new(HeroOptions::default());
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "# Overview of the API");

    assert_eq!(
        model.transcript_plain_items(),
        vec!["# Overview of the API"]
    );
}

#[test]
fn native_agent_request_includes_full_transcript_history() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::User, "first question");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "first answer");

    for character in "follow up".chars() {
        model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char(character),
        )));
    }
    let effect = model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Enter,
    )));

    let Some(AppEffect::SendNativeAgent { request }) = effect else {
        panic!("expected native agent effect, got {effect:?}");
    };
    let roles_and_content = request
        .llm_request()
        .messages
        .iter()
        .map(|message| (message.role.as_str(), message.content.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        roles_and_content,
        vec![
            ("user", "first question"),
            ("assistant", "first answer"),
            ("user", "follow up"),
        ]
    );
}

#[test]
fn native_agent_request_excludes_runtime_system_messages() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.append_system_message_from_runtime("Chat failed: connection refused");

    for character in "hello".chars() {
        model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char(character),
        )));
    }
    let effect = model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Enter,
    )));

    let Some(AppEffect::SendNativeAgent { request }) = effect else {
        panic!("expected native agent effect, got {effect:?}");
    };
    let roles_and_content = request
        .llm_request()
        .messages
        .iter()
        .map(|message| (message.role.as_str(), message.content.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(roles_and_content, vec![("user", "hello")]);
    assert_eq!(
        model.transcript_plain_items(),
        vec![
            "■ Chat failed: connection refused".to_string(),
            "› hello".to_string()
        ]
    );
}

#[test]
fn at_file_picker_opens_and_tab_completes_common_prefix() {
    let root = TempFileTree::new("tab-complete");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("README.md");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@s");

    assert!(model.file_picker_active());
    assert_eq!(
        model
            .current_file_picker_render_result()
            .plain_lines
            .iter()
            .filter(|line| line.contains("src/"))
            .count(),
        2
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    assert_eq!(model.composer_text(), "@src/");
}

#[test]
fn at_file_picker_tab_completes_prefix_candidates_not_fuzzy_matches() {
    let root = TempFileTree::new("tab-complete-prefix-only");
    root.write_file("src/dir1/docs.md");
    root.write_file("src/dir1/readme.md");
    root.write_file("docs/status.md");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@s");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    assert_eq!(model.composer_text(), "@src/dir1/");
}

#[test]
fn file_picker_render_shifts_to_the_completed_directory_prefix() {
    let root = TempFileTree::new("tab-complete-shifts-window");
    root.write_file("src/dir1/dir2/dir3/docs.md");
    root.write_file("src/dir1/dir2/dir4/readme.md");
    root.write_file("docs/status.md");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@s");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    assert_eq!(model.composer_text(), "@src/dir1/dir2/dir");

    let lines = model.current_file_picker_render_result().plain_lines;
    assert!(
        lines.iter().any(|line| line.contains("dir3/docs.md")),
        "completed directory prefix should be stripped from picker rows: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.contains("src/dir1/dir2")),
        "picker rows should not waste popup width on the already inserted prefix: {lines:?}"
    );
}

#[test]
fn file_picker_popup_uses_configured_height_and_full_width() {
    let root = TempFileTree::new("configured-popup-height");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("src/model.rs");
    root.write_file("src/view.rs");
    root.write_file("src/update.rs");

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 3,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@s");

    let rows = rendered_rows_for_model(&mut model, 40, 8);

    assert_eq!(
        rows.iter().filter(|line| line.contains("src/")).count(),
        3,
        "configured popup height should limit visible picker rows: {rows:?}"
    );
    assert!(
        rendered_segment(&rows[1], 0, 40)
            .trim_start()
            .starts_with("src/"),
        "file picker popup should render as a full-width row instead of a narrow anchored column: {rows:?}"
    );
}

#[test]
fn file_picker_popup_renders_vertical_scrollbar_for_overflowing_results() {
    let root = TempFileTree::new("popup-scrollbar");
    for index in 0..8 {
        root.write_file(&format!("src/file_{index:02}.rs"));
    }

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 3,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@s");

    let rows = rendered_rows_for_model(&mut model, 40, 8);
    let popup_rows = &rows[1..=3];

    assert!(
        popup_rows
            .iter()
            .any(|row| rendered_segment(row, 39, 1) == "█"),
        "overflowing file picker should render a thicker right-side scrollbar thumb: {rows:?}"
    );
    assert!(
        !popup_rows
            .iter()
            .any(|row| row.contains('↑') || row.contains('↓')),
        "file picker scrollbar should not render begin/end arrow symbols: {rows:?}"
    );
}

#[test]
fn at_file_picker_enter_inserts_selected_path_with_prefix_and_space() {
    let root = TempFileTree::new("enter-selected");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@src/l");

    assert!(model.file_picker_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "@src/lib.rs ");
    assert!(!model.file_picker_active());
}

#[test]
fn at_file_picker_down_then_enter_inserts_the_selected_path() {
    let root = TempFileTree::new("down-enter-selected");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@src");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "@src/main.rs ");
    assert!(!model.file_picker_active());
}

#[test]
fn at_file_picker_enter_on_empty_results_does_not_send_composer() {
    let root = TempFileTree::new("enter-empty-results");
    root.write_file("src/lib.rs");

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            model_catalog: file_picker_test_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@does-not-exist");

    assert!(model.file_picker_active());
    assert!(
        model
            .current_file_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("No files"))
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert_eq!(model.composer_text(), "@does-not-exist");
    assert!(
        !model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("@does-not-exist")),
        "enter in an empty file picker must not submit the draft"
    );
}

#[test]
fn file_picker_does_not_clear_status_lines_outside_popup_area() {
    let root = TempFileTree::new("keep-status-lines");
    root.write_file("src/lib.rs");

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 3,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.git_branch = "main".to_string();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "one\ntwo\nthree\nfour\nfive\nsix\n@s");
    apply_scrolled_offset(&mut model, 1, true);

    let rows = rendered_rows_for_model(&mut model, 40, 8);

    assert!(
        rows.iter().any(|line| line.contains("src/lib.rs")),
        "file picker content should still be visible in the rendered frame: {rows:?}"
    );
    assert!(
        rows.iter().any(|line| line.contains("main")),
        "status line outside the popup area should not be cleared by the floating layer: {rows:?}"
    );
}

#[test]
fn file_picker_remains_visible_when_composer_uses_most_of_viewport() {
    let root = TempFileTree::new("long-composer-visible");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("src/model.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(36, 6);
    type_text(&mut model, "line one\nline two\nline three\nline four\n@s");

    let layout = model.build_document_layout();
    let rows = rendered_rows_for_model(&mut model, 36, 6);

    assert!(
        rows.iter().any(|line| line.contains("src/lib.rs")),
        "file picker row should stay visible in the rendered frame: {rows:?}"
    );

    assert_eq!(
        layout.composer_line_count,
        usize::from(model.composer.full_height()),
        "file picker should render as an overlay, not by clipping composer document content"
    );
}

#[test]
fn file_picker_remains_visible_when_composer_cursor_mode_fills_viewport() {
    let root = TempFileTree::new("cursor-mode-visible");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("src/model.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(36, 6);
    type_text(
        &mut model,
        "line one\nline two\nline three\nline four\nline five\nline six",
    );
    model.document_runtime.follow_bottom = false;
    model.sync_document_viewport_for_composer_cursor();

    type_text(&mut model, "\n@s");

    let layout = model.build_document_layout();
    let rows = rendered_rows_for_model(&mut model, 36, 6);

    assert!(
        rows.iter().any(|line| line.contains("src/lib.rs")),
        "file picker row should stay visible in cursor viewport mode: {rows:?}"
    );

    assert_eq!(
        layout.composer_line_count,
        usize::from(model.composer.full_height()),
        "file picker should not participate in document layout"
    );
}

#[test]
fn file_picker_overlay_does_not_clip_or_extend_the_composer_document_layout() {
    let root = TempFileTree::new("top-trigger-full-composer");
    root.write_file("line-target.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(36, 6);
    model
        .composer_mut()
        .set_text_for_test("line one\nline two\nline three\nline four\nline five\nline six");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model.sync_document_viewport_for_composer_cursor();

    type_text(&mut model, "@");

    let expected_composer_lines = usize::from(model.composer.full_height());
    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);
    let rows = rendered_rows_for_model(&mut model, 36, 6);

    assert_eq!(
        layout.composer_line_count, expected_composer_lines,
        "file picker overlay must not replace the full composer document with a clipped slice"
    );
    assert!(
        !viewport
            .plain_lines
            .iter()
            .any(|line| line.contains("line-target.rs")),
        "file picker overlay must not be part of the document viewport: {:?}",
        viewport.plain_lines
    );

    assert!(
        rows.iter().any(|line| line.contains("line-target.rs")),
        "file picker row should still be visible in the rendered frame: {rows:?}"
    );
}

#[test]
fn file_picker_overlay_does_not_shrink_the_composer_viewport() {
    let root = TempFileTree::new("overlay-keeps-composer-viewport");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("src/model.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(36, 6);
    model
        .composer_mut()
        .set_text_for_test("line one\nline two\nline three\nline four\nline five\nline six");
    model.sync_composer_height();
    let before_visible_height = model.composer.visible_height();

    type_text(&mut model, "\n@s");

    assert!(model.file_picker_active());
    assert_eq!(
        model.composer.visible_height(),
        before_visible_height,
        "file picker overlay should reserve document rows without shrinking the composer viewport"
    );
}

#[test]
fn file_picker_floating_layer_does_not_shrink_document_viewport() {
    let root = TempFileTree::new("floating-keeps-document-viewport");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");
    root.write_file("src/model.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(36, 6);
    type_text(
        &mut model,
        "line one\nline two\nline three\nline four\nline five\n@s",
    );

    assert!(model.file_picker_active());
    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);

    assert_eq!(
        viewport.lines.len(),
        model.document_viewport_height().min(layout.line_count()),
        "floating picker must not reserve or subtract document viewport rows"
    );
}

#[test]
fn file_picker_popup_anchors_below_the_at_token_with_full_width() {
    let root = TempFileTree::new("popup-anchor-below");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    type_text(&mut model, "@s");

    let rows = rendered_rows_for_model(&mut model, 40, 8);

    assert_eq!(
        rendered_column(&rows[1], "src/lib.rs"),
        Some(2),
        "file picker should keep vertical @ anchoring but use the full viewport width: {rows:?}"
    );
    assert!(
        rendered_segment(&rows[1], 0, 40).contains("src/lib.rs"),
        "file picker popup should own the whole viewport row: {rows:?}"
    );
}

#[test]
fn file_picker_popup_flips_above_when_there_is_no_room_below_anchor() {
    let root = TempFileTree::new("popup-anchor-above");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(40, 8);
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    type_text(&mut model, "one\ntwo\nthree\nfour\nfive\nsix\nseven\n@s");

    let rows = rendered_rows_for_model(&mut model, 40, 8);

    assert_eq!(
        rendered_column(&rows[0], "src/lib.rs"),
        Some(2),
        "file picker should flip above the @ row instead of appending below the composer: {rows:?}"
    );
}

#[test]
fn file_picker_popup_stays_full_width_when_anchor_is_near_right_edge() {
    let root = TempFileTree::new("popup-full-width-near-right");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    model.set_window(26, 8);
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    type_text(&mut model, "abcdefghijklmnopqrs @s");

    let rows = rendered_rows_for_model(&mut model, 26, 8);

    assert_eq!(
        rendered_column(&rows[1], "src/lib.rs"),
        Some(2),
        "file picker should use full-width rows instead of horizontal flip logic: {rows:?}"
    );
}

#[test]
fn file_picker_popup_clears_full_width_rows_over_underlying_content() {
    let root = TempFileTree::new("popup-clear-rect");
    root.write_file("src/lib.rs");

    let mut model = file_picker_model(root.path());
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    model
        .composer_mut()
        .set_text_for_test("@s\nunderlying one\nunderlying two");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model.sync_document_viewport_for_composer_cursor();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));

    let rows = rendered_rows_for_model(&mut model, 40, 8);

    assert_eq!(
        rendered_segment(&rows[2], 0, 40).trim(),
        "",
        "Clear should reset the whole viewport row before rendering short/empty picker rows: {rows:?}"
    );
    assert!(
        !rows[2].contains("underlying"),
        "underlying composer text inside the popup rect should be covered: {rows:?}"
    );
}

#[test]
fn file_picker_esc_restores_area_cleared_by_previous_floating_frame() {
    let root = TempFileTree::new("popup-esc-restore");
    root.write_file("src/lib.rs");

    let mut model = file_picker_model(root.path());
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    model
        .composer_mut()
        .set_text_for_test("@s\nunderlying one\nunderlying two");
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model.sync_document_viewport_for_composer_cursor();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    assert!(model.file_picker_active());

    let mut terminal =
        Terminal::new(TestBackend::new(40, 8)).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render active popup");
    let popup_rows = rendered_rows(terminal.backend().buffer());
    assert!(
        popup_rows.iter().any(|line| line.contains("src/lib.rs")),
        "first frame should render the file picker popup: {popup_rows:?}"
    );
    assert!(
        !popup_rows
            .iter()
            .any(|line| line.contains("underlying one")),
        "first frame should cover underlying content inside the popup rect: {popup_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.file_picker_active());
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render after closing popup");
    let restored_rows = rendered_rows(terminal.backend().buffer());

    assert!(
        restored_rows
            .iter()
            .any(|line| line.contains("underlying one")),
        "closing the popup should repaint the content previously cleared by Clear: {restored_rows:?}"
    );
}

#[test]
fn file_picker_esc_restores_flipped_popup_area_in_full_composer_viewport() {
    let root = TempFileTree::new("popup-esc-restore-full");
    root.write_file("src/lib.rs");

    let mut model = file_picker_model(root.path());
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    model.set_window(40, 8);
    type_text(
        &mut model,
        "line one\nline two\nline three\nline four\nline five\nline six\nline seven\n@s",
    );
    assert!(model.file_picker_active());

    let mut terminal =
        Terminal::new(TestBackend::new(40, 8)).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render active popup");
    let popup_rows = rendered_rows(terminal.backend().buffer());
    assert!(
        popup_rows.iter().any(|line| line.contains("src/lib.rs")),
        "first frame should render the flipped file picker popup: {popup_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.file_picker_active());
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render after closing popup");
    let restored_rows = rendered_rows(terminal.backend().buffer());

    assert!(
        restored_rows.iter().any(|line| line.contains("line two")),
        "closing a flipped popup should repaint the composer rows it cleared: {restored_rows:?}"
    );
    assert!(
        !restored_rows.iter().any(|line| line.contains("src/lib.rs")),
        "closed file picker content must not remain in the frame: {restored_rows:?}"
    );
}

#[test]
fn file_picker_esc_closes_overlay_without_moving_document_viewport() {
    let root = TempFileTree::new("popup-esc-keeps-viewport");
    root.write_file("src/lib.rs");

    let mut model = file_picker_model(root.path());
    model.status_line_items.clear();
    model.status_line_2_items.clear();
    model.set_window(40, 6);
    model.composer_mut().set_text_for_test(
        "@s\nline two\nline three\nline four\nline five\nline six\nline seven\nline eight",
    );
    model.composer_mut().move_to_begin_for_test();
    model.sync_composer_height();
    model.document_runtime.follow_bottom = false;
    model.sync_document_viewport_for_composer_cursor();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    assert!(model.file_picker_active());

    let before_document_offset = model.document_runtime.viewport_y;
    let before_composer_offset = model.composer.viewport_offset();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.file_picker_active());
    assert_eq!(
        model.document_runtime.viewport_y, before_document_offset,
        "closing a floating popup should not move the document viewport"
    );
    assert_eq!(
        model.composer.viewport_offset(),
        before_composer_offset,
        "closing a floating popup should not page the composer to the bottom"
    );
}

#[test]
fn file_picker_mouse_wheel_scrolls_document_without_moving_popup_list() {
    let root = TempFileTree::new("popup-wheel-passes-through");
    for index in 0..8 {
        root.write_file(&format!("src/file_{index:02}.rs"));
    }

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 3,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    for index in 0..12 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("history line {index}"));
    }
    model.sync_transcript_render();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@s");
    assert!(model.file_picker_active());

    let before_document_offset = model.document_runtime.viewport_y;
    let before_picker_scroll = model.file_picker.as_ref().map(|state| state.scroll);
    assert!(
        before_document_offset > 0,
        "fixture should start with scrollable document content"
    );

    model.update(AppEvent::MouseWheel { delta_lines: -3 });

    assert!(
        model.document_runtime.viewport_y < before_document_offset,
        "mouse wheel should keep scrolling the underlying document while the file picker is active"
    );
    assert_eq!(
        model.file_picker.as_ref().map(|state| state.scroll),
        before_picker_scroll,
        "mouse wheel should not move the file picker list viewport"
    );
}

#[test]
fn deleting_file_picker_trigger_after_mouse_wheel_keeps_manual_document_viewport() {
    let root = TempFileTree::new("popup-wheel-delete-trigger");
    root.write_file("src/lib.rs");

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 3,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    for index in 0..12 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("history line {index}"));
    }
    model.sync_transcript_render();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@");
    assert!(model.file_picker_active());

    let bottom_offset = model.document_runtime.viewport_y;
    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let scrolled_offset = model.document_runtime.viewport_y;
    assert!(
        scrolled_offset < bottom_offset,
        "fixture should manually scroll away from the bottom before deleting the trigger"
    );
    assert!(model.document_runtime.manual_scroll);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));

    assert!(!model.file_picker_active());
    assert_eq!(model.composer_text(), "");
    assert_eq!(
        model.document_runtime.viewport_y, scrolled_offset,
        "removing the @ trigger after a wheel scroll should not restore the viewport to the bottom"
    );
    assert!(
        model.document_runtime.manual_scroll,
        "closing the popup by deleting its trigger should keep the user's manual scroll state"
    );
}

#[test]
fn height_only_resize_keeps_transcript_render_stable() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta\ngamma\ndelta");
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("1\n2\n3\n4\n5\n6");
    model.sync_composer_height();

    let before_render_version = model.transcript_render_version;
    let before_render = Rc::clone(&model.transcript_render);
    let before_composer_height = model.composer.visible_height();

    model.set_window(20, 8);

    assert_eq!(
        model.transcript_render_version, before_render_version,
        "height-only resize should not trigger a transcript rerender"
    );
    assert!(
        Rc::ptr_eq(&before_render, &model.transcript_render),
        "height-only resize should keep reusing the current transcript render result"
    );
    assert!(
        model.composer.visible_height() > before_composer_height,
        "height-only resize should still update the tail/composer layout"
    );
}

#[test]
fn setting_the_same_palette_keeps_transcript_render_stable() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "alpha\nbeta");
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);

    let before_render_version = model.transcript_render_version;
    let before_render = Rc::clone(&model.transcript_render);

    model.set_palette(default_palette(), true);

    assert_eq!(
        model.transcript_render_version, before_render_version,
        "setting the same palette should not trigger a transcript rerender"
    );
    assert!(
        Rc::ptr_eq(&before_render, &model.transcript_render),
        "setting the same palette should keep the existing transcript render result"
    );
}

#[test]
fn current_visible_transcript_window_matches_actual_viewport_line_indices() {
    #[derive(Clone, Copy)]
    enum TailState {
        Plain,
        StatusLine,
        CommandPanel,
    }

    for (name, style_mode, height, composer_text, tail_state) in [
        ("plain draft", StyleMode::Ms, 6, "draft", TailState::Plain),
        (
            "status line with tall draft",
            StyleMode::Ms,
            6,
            "1\n2\n3\n4\n5\n6\n7\n8",
            TailState::StatusLine,
        ),
        (
            "command panel",
            StyleMode::Ms,
            6,
            "/",
            TailState::CommandPanel,
        ),
        ("framed draft", StyleMode::Cc, 3, "draft", TailState::Plain),
        (
            "framed tall draft",
            StyleMode::Cc,
            4,
            "1\n2\n3\n4\n5\n6",
            TailState::Plain,
        ),
    ] {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), style_mode);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..48 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("item {index}"));
        }
        model.set_window(24, height);
        model.set_palette(default_palette(), true);
        match tail_state {
            TailState::Plain => {}
            TailState::StatusLine => {
                model.status_line_items = vec![StatusLineItem::GitBranch];
                model.git_branch = "main".to_string();
            }
            TailState::CommandPanel => {}
        }
        model.composer_mut().set_text_for_test(composer_text);
        model.sync_command_panel_navigation();
        model.sync_composer_height();
        model.sync_transcript_render();
        model.sync_document_viewport_to_bottom();

        let layout = model.build_document_layout();
        let visible_transcript_indices = model
            .document_viewport_line_indices(&layout)
            .into_iter()
            .filter(|line_index| *line_index < layout.transcript_line_count)
            .collect::<Vec<_>>();
        let expected_window = visible_transcript_indices
            .first()
            .copied()
            .map(|start| (start, visible_transcript_indices.len()));

        assert_eq!(
            model.current_visible_transcript_window(layout.transcript_line_count),
            expected_window,
            "{name} should derive the warmed transcript window from the actual viewport line indices"
        );
    }
}

#[test]
fn sync_transcript_render_evicts_warmed_transcript_blocks_during_metrics_only_refresh() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("item {index}"));
    }

    model.sync_transcript_render();
    assert!(
        model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "metrics-only sync should keep transcript blocks cold before any viewport materialization"
    );

    model.document_runtime.transcript_cache = Default::default();
    let _snapshot = model.current_document_transcript_snapshot();
    assert!(
        !model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "document transcript snapshot should prewarm the visible transcript neighborhood"
    );

    model.sync_transcript_render();
    assert!(
        model
            .transcript
            .cached_screen_blocks_snapshot()
            .borrow()
            .is_empty(),
        "metrics-only refresh should evict warmed transcript blocks from the previous viewport snapshot"
    );
}

#[test]
fn current_visible_transcript_window_reresolves_manual_scroll_viewport_after_resize() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "target item");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "tail item");
    model.set_window(24, 4);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let target_document_line = (0..layout.line_count())
        .find(|&line_index| {
            layout.line_anchor_at(line_index).is_some_and(|anchor| {
                anchor.region == DocumentAnchorRegion::Transcript
                    && anchor.transcript.item_index == 1
            })
        })
        .expect("target item should exist in the initial transcript layout");
    let document_offset = target_document_line;
    model.apply_document_viewport_position(&layout, document_offset, 0, false, true);
    let preserved_viewport_state = model.current_document_viewport_state();

    model.set_window(12, 4);

    let transcript_line_count = model.transcript.item_metrics_index().line_count;
    let resized_layout = model.build_document_layout();
    let resized_target_document_line = (0..resized_layout.line_count())
        .find(|&line_index| {
            resized_layout
                .line_anchor_at(line_index)
                .is_some_and(|anchor| {
                    anchor.region == DocumentAnchorRegion::Transcript
                        && anchor.transcript.item_index == 1
                })
        })
        .expect("target item should still exist after resize");
    let expected_offset =
        preserved_viewport_state.resolve_offset(&resized_layout, model.document_viewport_height());
    let stale_offset = preserved_viewport_state.resolved_offset();
    let expected_window = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            expected_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < transcript_line_count)
        .collect::<Vec<_>>();
    let stale_window = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            stale_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < transcript_line_count)
        .collect::<Vec<_>>();
    let expected_window = expected_window
        .first()
        .copied()
        .map(|start| (start, expected_window.len()));

    assert_ne!(
        expected_offset, stale_offset,
        "test fixture should force manual-scroll restore to resolve a different offset after reflow (before={target_document_line}, after={resized_target_document_line})"
    );
    assert_ne!(
        stale_window
            .first()
            .copied()
            .map(|start| (start, stale_window.len())),
        expected_window,
        "test fixture should expose a mismatch between stale and re-resolved viewport windows"
    );
    assert_eq!(
        model.current_visible_transcript_window(transcript_line_count),
        expected_window,
        "manual-scroll prewarm should follow the re-resolved viewport that will be restored after resize"
    );
}

#[test]
fn current_visible_transcript_window_rebuilds_manual_scroll_index_when_reflow_keeps_line_count() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    model.transcript_mut().append_message(
            Sender::Assistant,
            "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega",
        );
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "target item");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "tail item");
    model.set_window(24, 4);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let layout = model.build_document_layout();
    let target_document_line = (0..layout.line_count())
        .find(|&line_index| {
            layout.line_anchor_at(line_index).is_some_and(|anchor| {
                anchor.region == DocumentAnchorRegion::Transcript
                    && anchor.transcript.item_index == 1
            })
        })
        .expect("target item should exist in the initial transcript layout");
    model.apply_document_viewport_position(&layout, target_document_line, 0, false, true);

    let preserved_viewport_state = model.current_document_viewport_state();
    let stale_index = model.transcript_render.index.clone();
    model.width = 12;
    model.transcript.set_width(12);
    model.composer.set_width(12);
    let resized_index = model.transcript.progressive_item_metrics_index();
    let resized_layout = model.document_layout_for_transcript_index(resized_index.clone());
    let expected_offset =
        preserved_viewport_state.resolve_offset(&resized_layout, model.document_viewport_height());
    let expected_window_lines = model
        .document_viewport_line_indices_for_mode(
            &resized_layout,
            expected_offset,
            preserved_viewport_state.follow_bottom(),
            preserved_viewport_state.manual_scroll(),
        )
        .into_iter()
        .filter(|line_index| *line_index < resized_index.line_count)
        .collect::<Vec<_>>();
    let expected_window = expected_window_lines
        .first()
        .copied()
        .map(|start| (start, expected_window_lines.len()));
    let forced_stale_index = crate::frontend::tui::transcript::TranscriptItemMetricsIndex {
        line_count: resized_index.line_count,
        ..stale_index
    };
    model.transcript_render = Rc::new(index_only_render_result(forced_stale_index));

    assert_eq!(
        model.current_visible_transcript_window(resized_index.line_count),
        expected_window,
        "line_count equality alone should not let manual-scroll reuse a stale transcript index after reflow"
    );
}

#[test]
fn sync_transcript_render_keeps_transcript_blocks_cold_when_document_viewport_is_unavailable() {
    #[derive(Clone, Copy)]
    enum ViewportState {
        MissingWindow,
        ZeroHeight,
    }

    for viewport_state in [ViewportState::MissingWindow, ViewportState::ZeroHeight] {
        let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
        model.set_window(24, 6);
        model.set_palette(default_palette(), true);
        model.transcript_mut().clear();
        model.transcript_mut().set_gap(0);
        for index in 0..96 {
            model
                .transcript_mut()
                .append_message(Sender::Assistant, format!("item {index}"));
        }

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "Phase E sync_transcript_render should stop after metrics rebuild even while the viewport is still available"
        );

        model.document_runtime.transcript_cache = Default::default();
        let _snapshot = model.current_document_transcript_snapshot();
        assert!(
            !model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "test fixture should warm transcript blocks before making the viewport unavailable"
        );

        match viewport_state {
            ViewportState::MissingWindow => {
                model.has_window = false;
            }
            ViewportState::ZeroHeight => {
                model.height = 0;
            }
        }

        assert_eq!(model.document_viewport_height(), 0);
        let transcript_line_count = model.transcript.item_metrics_index().line_count;
        assert_eq!(
            model.current_visible_transcript_window(transcript_line_count),
            None,
            "unavailable viewport should not report any transcript line as visible"
        );

        model.sync_transcript_render();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "sync_transcript_render should keep transcript blocks cold when no viewport is available"
        );

        model.document_runtime.transcript_cache = Default::default();
        let _snapshot = model.current_document_transcript_snapshot();
        assert!(
            model
                .transcript
                .cached_screen_blocks_snapshot()
                .borrow()
                .is_empty(),
            "document transcript snapshots should not retain transcript blocks when no viewport is available"
        );
    }
}

#[test]
fn sync_transcript_render_keeps_current_viewport_exact_without_settling_entire_transcript() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);

    model.sync_transcript_render();

    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window(index.line_count)
        .expect("bottom-follow viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
    let (start_position, end_position) = index
        .summary_positions_for_line_window(start, count, overscan_lines)
        .expect("visible transcript window should resolve to summary positions");
    let exact_items = index.visible_items[start_position..=end_position]
        .iter()
        .map(|position| position.item_index)
        .collect::<Vec<_>>();

    assert!(
        !exact_items.is_empty(),
        "test fixture should expose at least one visible transcript item"
    );
    assert!(
        exact_items
            .iter()
            .all(|item_index| index.metrics[*item_index].is_exact()),
        "visible transcript window should be exact after sync_transcript_render"
    );
    assert!(
        index
            .metrics
            .iter()
            .enumerate()
            .any(|(item_index, metrics)| {
                !exact_items.contains(&item_index) && metrics.is_estimated()
            }),
        "progressive sync should leave non-visible transcript history estimated instead of settling the whole transcript"
    );
}

#[test]
fn composer_cursor_only_layout_refresh_reuses_long_composer_document() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Cx);
    model.set_window(80, 24);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .replace_text_and_move_to_end("中英 mixed long composer text ".repeat(120));
    model.sync_composer_height();
    let _ = model.build_document_layout();

    crate::frontend::tui::composer::reset_render_document_call_count();
    model.composer_mut().move_to_begin();
    model.sync_document_viewport_for_composer_cursor();
    let _ = model.build_document_layout();

    assert_eq!(
        crate::frontend::tui::composer::render_document_call_count(),
        0,
        "cursor-only layout refresh should reuse the cached long composer document"
    );
}

#[test]
fn sync_transcript_render_does_not_schedule_idle_history_refinement() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..32 {
        model.transcript_mut().append_message(
            Sender::User,
            format!(
                "message {index}: {}",
                "long user text should stay estimated unless it enters the viewport ".repeat(10)
            ),
        );
    }
    model.set_window(24, 6);
    model.set_palette(default_palette(), true);

    model.sync_transcript_render();

    assert!(
        model.next_timeout_deadline().is_none(),
        "sync should not install a background timer that competes with scroll input"
    );
}

#[test]
fn build_document_layout_exactizes_a_newly_scrolled_transcript_window() {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..96 {
        model.transcript_mut().append_message(
            Sender::Assistant,
            format!(
                "item {index}: alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu"
            ),
        );
    }
    model.set_window(18, 6);
    model.set_palette(default_palette(), true);
    model.sync_transcript_render();

    let tail_layout = model.build_document_layout();
    model.apply_document_viewport_position(&tail_layout, 0, 0, false, true);

    let _top_layout = model.build_document_layout();
    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window(index.line_count)
        .expect("manually scrolled viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);

    assert!(
        index.line_window_is_exact(start, count, overscan_lines),
        "building a layout for a newly scrolled viewport should exactize that transcript window before document rendering"
    );
    assert!(
        index
            .metrics
            .iter()
            .enumerate()
            .any(|(item_index, metrics)| { item_index > 16 && metrics.is_estimated() }),
        "scroll-driven exactization should stay local instead of settling the whole transcript"
    );
}

#[test]
fn build_document_layout_stable_exactization_loop_keeps_visible_window_exact() {
    let base = progressive_exactization_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());

    for manual_scroll in [false, true] {
        for offset in 0..=max_offset {
            let mut model = base.clone();
            apply_scrolled_offset(&mut model, offset, manual_scroll);

            let index = model.transcript_render.index.clone();
            let Some((start, count)) = model.current_visible_transcript_window_for_index(&index)
            else {
                continue;
            };
            let overscan_lines =
                crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                continue;
            }

            let index = model.exactize_visible_transcript_window_until_stable(index);
            let Some((next_start, next_count)) =
                model.current_visible_transcript_window_for_index(&index)
            else {
                continue;
            };
            let next_overscan_lines =
                crate::frontend::tui::transcript::viewport_overscan_line_budget(next_count);
            assert!(
                index.line_window_is_exact(next_start, next_count, next_overscan_lines),
                "stable exactization should converge the visible transcript window to exact metrics at offset {offset} (manual_scroll={manual_scroll})"
            );
        }
    }
}

#[test]
fn exactize_line_window_keeps_manual_scroll_window_local_after_reflow() {
    let mut model = progressive_exactization_fixture();
    let offset = 10;
    apply_scrolled_offset(&mut model, offset, true);

    let index = model.transcript_render.index.clone();
    let (start, count) = model
        .current_visible_transcript_window_for_index(&index)
        .expect("manual-scroll viewport should expose a visible transcript window");
    let overscan_lines = crate::frontend::tui::transcript::viewport_overscan_line_budget(count);
    assert!(
        !index.line_window_is_exact(start, count, overscan_lines),
        "test fixture should keep manual offset {offset} on the progressive path before render-time exactization"
    );

    let expected_item_range = index
        .item_range_for_line_window(start, count, overscan_lines)
        .expect("visible transcript window should resolve to an item range");
    let actual_item_range = model
        .transcript
        .exactize_line_window(start, count, overscan_lines)
        .expect("exactization should cover the visible transcript items");

    assert_eq!(
        actual_item_range, expected_item_range,
        "exactize_line_window should only exactize the item range resolved for the requested line window before reflow"
    );
}

#[test]
fn build_document_layout_keeps_manual_scroll_viewport_stable_without_exactization_reflow() {
    let base = progressive_exactization_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());

    for offset in 0..=max_offset {
        let mut model = base.clone();
        apply_scrolled_offset(&mut model, offset, true);
        let preserved_viewport_state = model.document_runtime.viewport_state.clone();

        let layout = model.build_document_layout();
        let expected_offset =
            preserved_viewport_state.resolve_offset(&layout, model.document_viewport_height());
        let viewport = model.build_document_viewport(&layout);

        assert_eq!(
            model.document_runtime.viewport_y, expected_offset,
            "manual-scroll viewport should stay aligned with the preserved transcript anchor at offset {offset}"
        );
        assert_eq!(
            model.document_runtime.viewport_state.resolved_offset(),
            expected_offset,
            "viewport state should store the stable manual-scroll offset at offset {offset}"
        );
        assert_eq!(
            viewport.resolved_offset, expected_offset,
            "document viewport materialization should keep using the resolved manual-scroll offset at offset {offset}"
        );
    }
}

#[test]
fn build_document_layout_resyncs_idle_viewport_after_exactization_reflow() {
    let base = idle_refinement_fixture();
    let layout = base.clone().build_document_layout();
    let max_offset = layout
        .line_count()
        .saturating_sub(base.document_viewport_height());
    let mut candidate = None;

    for offset in 0..=max_offset {
        let mut probe = base.clone();
        apply_scrolled_offset(&mut probe, offset, false);
        if probe.document_runtime.follow_bottom || probe.document_runtime.manual_scroll {
            continue;
        }

        let stale_offset = probe.document_runtime.viewport_state.resolved_offset();
        let mut exactized = probe.clone();
        let layout = exactized.build_document_layout();
        let cursor_hidden_with_stale_offset = layout.cursor_y < stale_offset
            || layout.cursor_y >= stale_offset.saturating_add(exactized.document_viewport_height());

        let mut expected = exactized.clone();
        expected.sync_document_viewport_for_composer_cursor();
        if cursor_hidden_with_stale_offset && expected.document_runtime.viewport_y != stale_offset {
            candidate = Some(offset);
            break;
        }
    }

    let offset = candidate.expect(
            "test fixture should expose a non-follow-bottom viewport whose stale offset hides the composer cursor after render-time exactization",
        );

    let mut model = base;
    apply_scrolled_offset(&mut model, offset, false);

    let mut expected = model.clone();
    let _ = expected.build_document_layout();
    expected.sync_document_viewport_for_composer_cursor();

    let layout = model.build_document_layout();
    let viewport = model.build_document_viewport(&layout);

    assert_eq!(
        model.document_runtime.viewport_y, expected.document_runtime.viewport_y,
        "render-time exactization should immediately rerun the idle viewport cursor sync"
    );
    assert_eq!(
        model.composer.viewport_offset(),
        expected.composer.viewport_offset(),
        "composer viewport should stay aligned with the cursor-tracking sync after exactization"
    );
    assert_eq!(
        model.document_runtime.viewport_state.resolved_offset(),
        expected.document_runtime.viewport_y,
        "viewport state should store the cursor-tracking offset after exactization"
    );
    assert_eq!(
        viewport.resolved_offset, expected.document_runtime.viewport_y,
        "document viewport materialization should use the cursor-tracking offset after exactization"
    );
    assert!(
        layout.cursor_y >= viewport.resolved_offset
            && layout.cursor_y
                < viewport
                    .resolved_offset
                    .saturating_add(model.document_viewport_height()),
        "render-time exactization should leave the active composer cursor inside the visible document viewport"
    );
}

#[test]
fn acp_permission_accept_key_returns_selected_option() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-1".to_string(),
        title: Some("Write file".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    assert!(model.current_status_notice_text().is_empty());
    assert!(model.tool_approval_panel_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('y'))));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-1".to_string(),
            option_id: Some("allow-once".to_string()),
        })
    );
    assert!(model.current_status_notice_text().is_empty());
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn acp_permission_reject_key_returns_reject_option() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-2".to_string(),
        title: None,
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('n'))));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-2".to_string(),
            option_id: Some("reject-once".to_string()),
        })
    );
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn acp_permission_enter_on_session_allow_returns_allow_always_option() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-3".to_string(),
        title: Some("Run command".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-3".to_string(),
            option_id: Some("allow-always".to_string()),
        })
    );
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn acp_permission_enter_on_session_deny_returns_reject_always_option() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-4".to_string(),
        title: Some("Run command".to_string()),
        allow_option_id: Some("allow-once".to_string()),
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: Some("reject-once".to_string()),
        reject_always_option_id: Some("reject-always".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-4".to_string(),
            option_id: Some("reject-always".to_string()),
        })
    );
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn acp_permission_shortcuts_use_session_options_when_once_options_are_absent() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut model = Model::new(HeroOptions::default());
    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-5".to_string(),
        title: Some("Run command".to_string()),
        allow_option_id: None,
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: None,
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('y'))));
    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-5".to_string(),
            option_id: Some("allow-always".to_string()),
        })
    );

    model.update(AppEvent::AcpPermissionRequested {
        request_id: "permission-6".to_string(),
        title: Some("Run command".to_string()),
        allow_option_id: None,
        allow_always_option_id: Some("allow-always".to_string()),
        reject_option_id: None,
        reject_always_option_id: Some("reject-always".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('n'))));
    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-6".to_string(),
            option_id: Some("reject-always".to_string()),
        })
    );
}

#[test]
fn stream_activity_line_uses_dynamic_codex_style_indicator() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Cooking".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity("Kimi Code CLI");

    let first = model
        .current_stream_activity_render_result_at(std::time::Instant::now())
        .plain_line;
    let second = model
        .current_stream_activity_render_result_at(
            std::time::Instant::now() + std::time::Duration::from_millis(700),
        )
        .plain_line;

    assert!(first.contains("Cooking (0s"));
    assert!(first.contains("esc 2x to interrupt"));
    assert!(first.starts_with("• Cooking (0s"));
    assert!(!first.contains("Kimi Code CLI"));
    assert!(!first.contains('⠋'));
    assert_eq!(first, second);
}

#[test]
fn stream_activity_line_cycles_configured_fallback_phrases() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Cooking".to_string(), "Crafting".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.set_window(50, 6);
    model.set_palette(default_palette(), true);

    model.show_stream_activity("qwen3");
    let first = model.current_stream_activity_render_result().plain_line;
    model.clear_stream_activity();
    model.show_stream_activity("qwen3");
    let second = model.current_stream_activity_render_result().plain_line;

    assert!(first.contains("Cooking (0s"));
    assert!(second.contains("Crafting (0s"));
}

#[test]
fn stream_activity_line_renders_above_composer() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Cooking".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.set_window(40, 6);
    model.set_palette(default_palette(), true);
    model.show_stream_activity("Kimi Code CLI");

    let layout = model.build_document_layout();

    let activity_line = layout
        .tail
        .text_lines
        .first()
        .map(|line| line.trim())
        .unwrap_or_default();
    assert!(activity_line.contains("Cooking"));
    assert!(!activity_line.contains("Kimi Code CLI"));
    assert_eq!(
        layout.tail.text_lines.get(1).map(String::as_str),
        Some(""),
        "activity indicator should breathe before the composer"
    );
    assert_eq!(layout.composer_slot.frame_start_line, 2);
    assert!(layout.composer_slot.content_start_line > layout.composer_slot.frame_start_line);
}

#[test]
fn esc_interrupts_native_agent_after_configured_press_count() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 2,
            ..ModelOptions::default()
        },
    );
    model.show_stream_activity("qwen3");

    let first = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(first, None);
    assert!(model.current_status_notice_text().contains("Esc again"));
    assert!(model.current_stream_activity_render_result().has_content);

    let second = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(second, Some(AppEffect::InterruptCurrentTurn));
}

#[test]
fn esc_interrupts_native_agent_immediately_when_configured_for_one_press() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 1,
            ..ModelOptions::default()
        },
    );
    model.show_stream_activity("qwen3");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, Some(AppEffect::InterruptCurrentTurn));
}

#[test]
fn enter_during_native_agent_activity_does_not_append_unsent_message() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.composer_mut().insert_text("second message");
    model.show_stream_activity("qwen3");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(
        model
            .current_status_notice_text()
            .contains("already running")
    );
    assert_eq!(model.composer_text(), "second message");
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn enter_acp_prompt_starts_activity_before_worker_ack() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Submitted".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.composer_mut().insert_text("hello acp");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SendAcpPrompt {
            agent_id: "Kimi Code CLI".to_string(),
            prompt: crate::runtime::acp::AcpPrompt::from_text("hello acp"),
        })
    );
    assert!(
        model
            .current_stream_activity_render_result()
            .plain_line
            .contains("Submitted (0s")
    );
}

#[test]
fn enter_acp_prompt_builds_structured_blocks_from_selected_agent_capabilities() {
    use agent_client_protocol::schema::{
        AgentCapabilities, ContentBlock, EmbeddedResourceResource, PromptCapabilities,
    };

    let root = TempFileTree::new("acp-structured-prompt");
    root.write_file_with_content("assets/sample.png", &[0x89, b'P', b'N', b'G']);
    root.write_file_with_content("src/code.py", b"print('hi')\n");

    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_phrases: vec!["Submitted".to_string()],
            status_phrase_order: StatusPhraseOrder::Cycle,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model.selected_acp_agent = Some("fake".to_string());
    model.apply_acp_agent_identity(
        "fake",
        crate::runtime::acp::AcpAgentIdentity {
            agent_capabilities: AgentCapabilities::new()
                .prompt_capabilities(PromptCapabilities::new().image(true).embedded_context(true)),
            ..crate::runtime::acp::AcpAgentIdentity::default()
        },
    );
    model
        .composer_mut()
        .insert_text("review @assets/sample.png @src/code.py");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let Some(AppEffect::SendAcpPrompt { agent_id, prompt }) = effect else {
        panic!("expected structured ACP prompt effect");
    };

    assert_eq!(agent_id, "fake");
    let blocks = prompt.to_content_blocks();
    assert_eq!(blocks.len(), 4);
    assert!(matches!(
        &blocks[1],
        ContentBlock::Image(image)
            if image.mime_type == "image/png"
                && image.data == "iVBORw=="
                && image.uri.as_deref() == Some(root.file_uri("assets/sample.png").as_str())
    ));
    match &blocks[3] {
        ContentBlock::Resource(resource) => match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(text) => {
                assert_eq!(text.uri, root.file_uri("src/code.py"));
                assert_eq!(text.text, "print('hi')\n");
            }
            other => panic!("expected embedded text resource, got {other:?}"),
        },
        other => panic!("expected resource block, got {other:?}"),
    }
}

#[test]
fn current_model_status_line_falls_back_to_selected_acp_agent() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            status_line_items: vec![StatusLineItem::CurrentModel],
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            model_catalog: ModelCatalog::new(vec![ModelProvider::native(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            ..ModelOptions::default()
        },
    );

    model.selected_acp_agent = Some("Kimi Code CLI".to_string());

    assert_eq!(
        model.current_status_line_parts(),
        vec!["Kimi Code CLI".to_string()]
    );
}

#[test]
fn stream_activity_line_shows_interrupt_hint() {
    let mut model = Model::new(HeroOptions::default());
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());

    model.show_stream_activity("Kimi Code CLI");
    let line = model.current_stream_activity_render_result().plain_line;

    assert!(line.contains("esc 2x to interrupt"));
}

#[test]
fn esc_interrupts_stream_activity_after_configured_press_count() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 2,
            ..ModelOptions::default()
        },
    );
    model.selected_acp_agent = Some("Kimi Code CLI".to_string());
    model.show_stream_activity("Kimi Code CLI");

    let first = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(first, None);
    assert!(model.current_status_notice_text().contains("Esc again"));

    let second = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(second, Some(AppEffect::InterruptCurrentTurn));
}

#[test]
fn esc_interrupt_count_resets_when_interrupt_notice_expires() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 2,
            ..ModelOptions::default()
        },
    );
    model.show_stream_activity("qwen3");

    let first = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(first, None);
    assert!(model.current_status_notice_text().contains("Esc again"));

    let timeout = model
        .timeout_event(std::time::Instant::now() + std::time::Duration::from_secs(2))
        .expect("interrupt notice should time out");
    assert_eq!(model.update(timeout), None);
    assert!(model.current_status_notice_text().is_empty());

    let second_after_timeout = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(second_after_timeout, None);
    assert!(model.current_status_notice_text().contains("Esc again"));

    let third = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(third, Some(AppEffect::InterruptCurrentTurn));
}

#[test]
fn stream_activity_line_can_hide_interrupt_hint() {
    let mut model = Model::new_with_options(
        HeroOptions::default(),
        ModelOptions {
            show_esc_interrupt_hint: false,
            ..ModelOptions::default()
        },
    );

    model.show_stream_activity_with_header("Working");
    let line = model.current_stream_activity_render_result().plain_line;

    assert!(line.contains("Working (0s)"));
    assert!(!line.contains("esc"));
    assert!(!line.contains("interrupt"));
}

fn file_picker_model(root: &Path) -> Model {
    let mut model = Model::new_with_style_mode(HeroOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.current_dir = root.display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model
}

fn file_picker_test_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::native(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}

fn type_text(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
}

fn rendered_rows_for_model(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(width, height)).expect("test backend should initialize");
    terminal
        .draw(|frame| model.render(frame))
        .expect("model should render into test backend");
    rendered_rows(terminal.backend().buffer())
}

fn rendered_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            let mut line = String::new();
            for column in 0..buffer.area.width {
                line.push_str(buffer[(column, row)].symbol());
            }
            line
        })
        .collect()
}

fn rendered_column(row: &str, needle: &str) -> Option<usize> {
    row.find(needle)
        .map(|byte_index| row[..byte_index].chars().count())
}

fn rendered_segment(row: &str, start: usize, len: usize) -> String {
    row.chars().skip(start).take(len).collect()
}

struct TempFileTree {
    path: PathBuf,
}

impl TempFileTree {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("lumos-file-picker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp root should be creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write_file(&self, relative: &str) {
        self.write_file_with_content(relative, b"");
    }

    fn write_file_with_content(&self, relative: &str, content: &[u8]) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("temp parent should be creatable");
        }
        std::fs::write(path, content).expect("temp file should be writable");
    }

    fn file_uri(&self, relative: &str) -> String {
        url::Url::from_file_path(self.path.join(relative))
            .expect("temp file path should convert to URI")
            .to_string()
    }
}

impl Drop for TempFileTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
