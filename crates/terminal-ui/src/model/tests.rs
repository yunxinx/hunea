use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use super::*;

use crate::{
    AppEffect, AppEvent, Sender, StyleMode,
    document::DocumentAnchorRegion,
    test_helpers::{render_model_buffer, rendered_rows},
    toast::ToastSeverity,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Modifier;
use runtime_domain::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use runtime_domain::phrases::StatusPhraseOrder;
use runtime_domain::prompt_assembly::PromptSourceOrigin;
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScope;
use runtime_domain::prompt_assembly::{
    PromptAssemblyDiscoveredSkill, PromptAssemblyExtraPromptCandidate, PromptAssemblyInput,
    PromptAssemblyManagerSnapshot, PromptPreludeSnapshot, resolve_prompt_assembly,
};
use runtime_domain::provider::ProviderKind;
use std::path::{Path, PathBuf};

mod message_history;

fn progressive_exactization_fixture() -> Model {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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

#[test]
fn startup_banner_entrance_runs_only_once_across_banner_rebuilds() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    model.set_palette(crate::theme::default_palette(), true);

    let started_at = Instant::now();
    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(started_at),
        None
    );

    model.start_startup_banner_entrance_for_test(started_at);
    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(started_at),
        Some(Duration::from_millis(16))
    );

    model.complete_startup_banner_entrance_for_test();
    assert!(model.startup_banner_entrance_completed_for_test());
    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(started_at),
        None
    );

    model.reset_to_initial_tui_state();

    assert!(model.startup_banner_entrance_completed_for_test());
    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(started_at),
        None
    );
}

#[test]
fn selection_copy_completion_uses_toast_not_status_notice() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.handle_selection_copy_completed(true);

    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(model.active_toast_text_for_test(), Some("Selection copied"));
}

#[test]
fn send_validation_error_uses_toast_not_status_notice() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
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
    model.composer_mut().insert_text("hello");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Select a model before sending")
    );
    assert_eq!(model.composer_text(), "hello");
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .all(|item| !item.contains("hello"))
    );
}

#[test]
fn exit_confirmation_stays_on_status_line_when_toast_exists() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.show_toast(ToastSeverity::Info, "Background notice");

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(model.current_status_notice_text().contains("exit"));
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Background notice")
    );
}

#[test]
fn model_render_buffer_contains_keycap_grapheme() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("2️⃣");
    model.sync_composer_height();

    let buffer = render_model_buffer(&mut model, 20, 4);

    assert!(
        buffer.content().iter().any(|cell| cell.symbol() == "2️⃣"),
        "keycap emoji should be written as one visible wide grapheme"
    );
}

#[test]
fn model_render_buffer_replaces_placeholder_when_keycap_draft_appears() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.set_window(32, 4);
    model.set_palette(default_palette(), true);
    model.sync_composer_height();

    let placeholder_buffer = render_model_buffer(&mut model, 32, 4);
    let placeholder_frame = rendered_rows(&placeholder_buffer).join("\n");
    assert!(
        placeholder_frame.contains("Enter to send Prompt"),
        "empty composer should render the placeholder before replacement: {placeholder_frame:?}"
    );

    model.composer_mut().set_text_for_test("2️⃣");
    model.sync_composer_height();
    let keycap_buffer = render_model_buffer(&mut model, 32, 4);
    let keycap_frame = rendered_rows(&keycap_buffer).join("\n");

    assert!(keycap_frame.contains("2️⃣"));
    assert!(
        !keycap_frame.contains("Enter to send Prompt"),
        "replacing placeholder with a keycap emoji must not leave stale placeholder cells: {keycap_frame:?}"
    );
}

#[test]
fn model_render_keeps_keycap_trailing_cell_in_ratatui_shape() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.transcript_mut().clear();
    model.set_window(20, 4);
    model.set_palette(default_palette(), true);
    model.composer_mut().set_text_for_test("2️⃣");
    model.sync_composer_height();

    let buffer = render_model_buffer(&mut model, 20, 4);
    let keycap_position = buffer
        .content()
        .iter()
        .enumerate()
        .find_map(|(index, cell)| {
            if cell.symbol() == "2️⃣" {
                Some(buffer.pos_of(index))
            } else {
                None
            }
        })
        .expect("keycap should be rendered");
    let trailing_cell = &buffer[(keycap_position.0 + 1, keycap_position.1)];

    assert_eq!(trailing_cell.symbol(), " ");
    assert_eq!(trailing_cell.fg, ratatui::style::Color::Reset);
    assert_eq!(trailing_cell.bg, ratatui::style::Color::Reset);
    assert_eq!(trailing_cell.underline_color, ratatui::style::Color::Reset);
    assert!(trailing_cell.modifier.is_empty());
}

#[test]
fn startup_banner_entrance_starts_after_ready_render() {
    let mut model = Model::new(StartupBannerOptions::default());
    let now = Instant::now();

    let _ = render_model_buffer(&mut model, 80, 24);
    assert_eq!(model.startup_banner_entrance_frame_interval_at(now), None);

    model.set_window(80, 24);
    model.set_palette(crate::theme::default_palette(), true);
    let _ = render_model_buffer(&mut model, 80, 24);

    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(now),
        Some(Duration::from_millis(16))
    );
}

#[test]
fn startup_banner_entrance_does_not_poll_while_transcript_overlay_hides_target() {
    let mut model = Model::new(StartupBannerOptions::default());
    let now = Instant::now();
    model.set_window(80, 24);
    model.set_palette(crate::theme::default_palette(), true);
    model.start_startup_banner_entrance_for_test(now);

    model.open_transcript_overlay();

    assert!(model.startup_banner_entrance_completed_for_test());
    assert_eq!(model.startup_banner_entrance_frame_interval_at(now), None);
}

#[test]
fn startup_banner_entrance_completes_when_transcript_overlay_renders_over_target() {
    let mut model = Model::new(StartupBannerOptions::default());
    let now = Instant::now();
    model.set_window(80, 24);
    model.set_palette(crate::theme::default_palette(), true);
    model.start_startup_banner_entrance_for_test(now);
    model.open_transcript_overlay();

    let _ = render_model_buffer(&mut model, 80, 24);

    assert!(model.startup_banner_entrance_completed_for_test());
    assert_eq!(model.startup_banner_entrance_frame_interval_at(now), None);
}

#[test]
fn startup_banner_entrance_completes_across_overlay_toggle() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.update(AppEvent::Resized {
        width: 20,
        height: 10,
    });
    model.update(AppEvent::StartupReadyTimeout);
    type_text(&mut model, "hi");
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let _startup_frame_rows = rendered_rows_for_model(&mut model, 20, 10);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.startup_banner_entrance_completed_for_test());
    let _overlay_rows = rendered_rows_for_model(&mut model, 20, 10);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let closed_rows = rendered_rows_for_model(&mut model, 20, 10);

    assert!(model.startup_banner_entrance_completed_for_test());
    assert_eq!(
        model.startup_banner_entrance_frame_interval_at(Instant::now()),
        None
    );
    assert!(
        closed_rows.iter().any(|row| row.contains("directory:")),
        "closed overlay should restore the normal document view: {closed_rows:?}"
    );
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

fn conversation_test_model() -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
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
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model
}

fn apply_scrolled_offset(model: &mut Model, offset: usize, manual_scroll: bool) {
    let layout = model.build_document_layout();
    let composer_offset = model.current_composer_viewport_offset(&layout, offset);
    model.apply_document_viewport_position(&layout, offset, composer_offset, false, manual_scroll);
}

#[test]
fn overflowing_document_bottom_slice_keeps_full_draft_height() {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new(StartupBannerOptions::default());
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
fn conversation_turn_request_carries_only_current_user_message() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
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

    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "follow up");
}

#[test]
fn conversation_turn_request_ignores_runtime_system_messages_in_transcript() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
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
    model.append_system_message_from_runtime("connection refused");

    for character in "hello".chars() {
        model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
            crossterm::event::KeyCode::Char(character),
        )));
    }
    let effect = model.update(AppEvent::Key(crossterm::event::KeyEvent::from(
        crossterm::event::KeyCode::Enter,
    )));

    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "hello");
    assert_eq!(
        model.transcript_plain_items(),
        vec!["■ connection refused".to_string(), "› hello".to_string()]
    );
}

#[test]
fn conversation_turn_request_preserves_at_file_reference_as_text() {
    let root = TempFileTree::new("conversation-structured-prompt");
    root.write_file_with_content("assets/sample.png", &[0x89, b'P', b'N', b'G']);
    root.write_file_with_content("src/code.py", b"print('hi')\n");

    let mut model = conversation_test_model();
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model
        .composer_mut()
        .insert_text("review @assets/sample.png @src/code.py");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect");
    };

    assert!(request.is_user_message());
    assert_eq!(
        request.message_text(),
        "review @assets/sample.png @src/code.py"
    );
}

#[test]
fn conversation_turn_request_does_not_reuse_structured_transcript_history() {
    let root = TempFileTree::new("conversation-structured-history");
    root.write_file_with_content("assets/sample.png", &[0x89, b'P', b'N', b'G']);

    let mut model = conversation_test_model();
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model
        .composer_mut()
        .insert_text("inspect @assets/sample.png");
    let first = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(matches!(
        first,
        Some(AppEffect::SendConversationTurn { .. })
    ));

    model
        .transcript_mut()
        .append_message(Sender::Assistant, "first answer");
    model.composer_mut().insert_text("follow up");
    let second = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let Some(AppEffect::SendConversationTurn { request, .. }) = second else {
        panic!("expected conversation turn effect");
    };

    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "follow up");
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
fn dollar_skill_picker_opens_and_enter_inserts_bound_skill_token() {
    let mut model = skill_picker_model();

    type_text(&mut model, "$co");

    assert!(model.skill_picker_active());
    assert!(
        model
            .current_skill_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("Code Review"))
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert_eq!(model.composer_text(), "$code-review ");
    assert!(!model.skill_picker_active());
    let source_message = model.composer.source_message();
    assert_eq!(source_message.skill_bindings().len(), 1);
    assert_eq!(source_message.skill_bindings()[0].skill_name, "code-review");
}

#[test]
fn hash_custom_prompt_picker_opens_and_enter_inserts_bound_prompt_token() {
    let mut model = custom_prompt_picker_model();

    type_text(&mut model, "#rev");

    assert!(model.has_current_floating_layer());
    let floating_rows = rendered_rows_for_model(&mut model, 40, 8);
    assert!(
        floating_rows
            .iter()
            .any(|row| row.contains("Review Rules") || row.contains("review-rules")),
        "custom prompt picker should render matching prompt rows: {floating_rows:?}"
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert_eq!(model.composer_text(), "#review-rules ");
    let source_message = model.composer.source_message();
    assert_eq!(source_message.custom_prompt_bindings().len(), 1);
    assert_eq!(
        source_message.custom_prompt_bindings()[0].reference_id,
        "review-rules"
    );
}

#[test]
fn custom_prompt_picker_reserves_description_column_like_skill_picker() {
    let mut model = custom_prompt_picker_model();
    model.set_window(24, 8);

    type_text(&mut model, "#");

    let lines = model
        .current_custom_prompt_picker_render_result()
        .plain_lines;
    let release_line = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .nth(1)
        .expect("release checklist row should render");

    assert_eq!(
        release_line.rfind("... (global)"),
        Some(12),
        "custom prompt picker should keep the description column aligned with the skill picker: {release_line:?}"
    );
}

#[test]
fn custom_prompt_picker_renders_body_summary_with_scope_suffix() {
    let mut model = custom_prompt_picker_model();
    model.set_window(80, 8);

    type_text(&mut model, "#rev");

    let lines = model
        .current_custom_prompt_picker_render_result()
        .plain_lines;
    let review_line = lines
        .iter()
        .find(|line| line.contains("Review Rules"))
        .expect("review rules row should render");

    assert!(
        review_line.contains("Review every diff for regressions and missing tests."),
        "custom prompt picker should show prompt content and scope in the description column: {review_line:?}"
    );
    assert!(
        review_line.trim_end().ends_with("(project)"),
        "custom prompt picker should pin the scope suffix at the row end: {review_line:?}"
    );
    assert!(
        !review_line.contains("#review-rules"),
        "custom prompt picker description should not fall back to the reference id when prompt content exists: {review_line:?}"
    );
}

#[test]
fn custom_prompt_picker_does_not_render_reference_id_when_prompt_has_only_title() {
    let mut model = custom_prompt_picker_model();
    model.prompt_assembly.extra_prompt_candidates[0].reference_id = "new-prompt-1".to_string();
    model.prompt_assembly.extra_prompt_candidates[0].title = "测试用的".to_string();
    model.prompt_assembly.extra_prompt_candidates[0].body = "# 测试用的\n".to_string();
    model.set_window(80, 8);

    type_text(&mut model, "#new");

    let lines = model
        .current_custom_prompt_picker_render_result()
        .plain_lines;
    let prompt_line = lines
        .iter()
        .find(|line| line.contains("测试用的"))
        .expect("title-only prompt row should render");

    assert!(
        !prompt_line.contains("#new-prompt-1"),
        "custom prompt picker should not fall back to the reference id when the prompt body only has a title: {prompt_line:?}"
    );
    assert!(
        prompt_line.trim_end().ends_with("(project)"),
        "custom prompt picker should still show the scope suffix for title-only prompts: {prompt_line:?}"
    );
}

#[test]
fn custom_prompt_picker_keeps_scope_suffix_visible_at_row_end_when_content_is_long() {
    let mut model = custom_prompt_picker_model();
    model.prompt_assembly.extra_prompt_candidates[0].body =
        "This is a deliberately long custom prompt body that should be clipped before the scope suffix disappears.\n".to_string();
    model.set_window(36, 8);

    type_text(&mut model, "#rev");

    let lines = model
        .current_custom_prompt_picker_render_result()
        .plain_lines;
    let review_line = lines
        .iter()
        .find(|line| line.contains("Review Rules"))
        .expect("review rules row should render");

    assert!(
        review_line.trim_end().ends_with("(project)"),
        "custom prompt picker should keep the scope suffix fixed at the row end when body content is long: {review_line:?}"
    );
}

#[test]
fn moving_cursor_back_onto_bound_skill_token_reopens_skill_picker() {
    let mut model = skill_picker_model();

    type_text(&mut model, "$co");
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    type_text(&mut model, "later");
    assert!(!model.skill_picker_active());

    for _ in 0.."later ".chars().count() {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Left)));
    }

    assert!(model.skill_picker_active());
    assert!(
        model
            .current_skill_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("Code Review"))
    );
}

#[test]
fn skill_picker_renders_title_and_description_in_separate_columns() {
    let mut model = skill_picker_model();

    type_text(&mut model, "$");

    let lines = model.current_skill_picker_render_result().plain_lines;
    let skill_line = lines
        .iter()
        .find(|line| line.contains("Code Review"))
        .expect("skill picker should render matching row");

    assert!(skill_line.contains("Review code"));
    assert!(!skill_line.contains("[Skill]"));
    assert!(!skill_line.contains("$code-review - Review code"));
}

#[test]
fn skill_picker_keeps_description_column_aligned_across_rows() {
    let mut model = skill_picker_model();

    type_text(&mut model, "$");

    let lines = model.current_skill_picker_render_result().plain_lines;
    let code_review_line = lines
        .iter()
        .find(|line| line.contains("Code Review"))
        .expect("code review row should render");
    let brainstorming_line = lines
        .iter()
        .find(|line| line.contains("Brainstorming"))
        .expect("brainstorming row should render");

    let code_review_description_column = code_review_line
        .find("Review code")
        .expect("code review description should render");
    let brainstorming_description_column = brainstorming_line
        .find("Explore intent")
        .expect("brainstorming description should render");

    assert_eq!(
        code_review_description_column, brainstorming_description_column,
        "description column should start at the same visual position across rows"
    );
}

#[test]
fn skill_picker_highlights_matched_description_text() {
    let mut model = skill_picker_model();
    model.set_window(80, 8);
    model.set_palette(default_palette(), true);

    type_text(&mut model, "$inspect");

    let rendered = model.current_skill_picker_render_result();
    let skill_line = rendered
        .lines
        .iter()
        .find(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("inspect") || span.content.contains("Inspect"))
        })
        .expect("matching skill line should render");

    let highlighted_span = skill_line
        .spans
        .iter()
        .find(|span| span.content.to_ascii_lowercase().contains("inspect"))
        .expect("matched description span should render separately");

    assert!(
        highlighted_span.style.bg == default_palette().surface
            || highlighted_span
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
        "matched description text should use background-like highlight: {:?}",
        highlighted_span.style
    );
}

#[test]
fn skill_picker_popup_scrollbar_thumb_reaches_bottom_on_last_page() {
    let mut model = overflowing_skill_picker_model();

    type_text(&mut model, "$");
    for _ in 0..9 {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    }

    let rows = rendered_rows_for_model(&mut model, 40, 8);
    let last_visible_skill_row = rows
        .iter()
        .rposition(|row| row.contains("Skill "))
        .expect("skill picker should render visible skill rows");

    assert_eq!(
        rendered_segment(&rows[last_visible_skill_row], 39, 1),
        "█",
        "skill picker scrollbar thumb should reach the bottom row on the last page: {rows:?}"
    );
}

#[test]
fn fullscreen_modal_closes_composer_file_picker_state() {
    let root = TempFileTree::new("fullscreen-closes-file-picker");
    root.write_file("src/lib.rs");

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@s");
    assert!(model.file_picker_active());

    model.open_session_picker_loading();

    assert!(!model.file_picker_active());
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
fn file_picker_highlights_matched_path_fragment() {
    let root = TempFileTree::new("file-picker-highlight");
    root.write_file("src/lib.rs");
    root.write_file("src/main.rs");

    let mut model = file_picker_model(root.path());
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@src/li");

    let rendered = model.current_file_picker_render_result();
    let file_line_index = rendered
        .plain_lines
        .iter()
        .position(|line| line.contains("lib.rs"))
        .expect("matching file row should render");
    let file_line = &rendered.lines[file_line_index];

    let highlighted_span = file_line
        .spans
        .iter()
        .find(|span| span.content == "li")
        .expect("matched path fragment should render separately");

    assert!(
        highlighted_span.style.bg == default_palette().surface
            || highlighted_span
                .style
                .add_modifier
                .contains(Modifier::REVERSED),
        "matched path fragment should use background-like highlight: {:?}",
        highlighted_span.style
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
        StartupBannerOptions::default(),
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
        StartupBannerOptions::default(),
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
fn file_picker_popup_scrollbar_thumb_reaches_bottom_on_last_page() {
    let root = TempFileTree::new("popup-scrollbar-last-page");
    for index in 0..10 {
        root.write_file(&format!("src/file_{index:02}.rs"));
    }

    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Ms,
            file_picker_popup_height: 5,
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.current_dir = root.path().display().to_string();
    model.set_window(40, 10);
    model.set_palette(default_palette(), true);
    type_text(&mut model, "@s");
    for _ in 0..9 {
        let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    }

    let rows = rendered_rows_for_model(&mut model, 40, 10);
    let last_visible_file_row = rows
        .iter()
        .rposition(|row| row.contains("src/file_"))
        .expect("file picker should render visible file rows");

    assert_eq!(
        rendered_segment(&rows[last_visible_file_row], 39, 1),
        "█",
        "file picker scrollbar thumb should reach the bottom row on the last page: {rows:?}"
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
fn at_file_picker_enter_inserts_image_placeholder_and_attachment() {
    let root = TempFileTree::new("enter-selected-image");
    root.write_file_with_content("assets/sample.png", valid_png_header());
    root.write_file("src/main.rs");

    let mut model = conversation_test_model();
    model.current_dir = root.path().display().to_string();
    type_text(&mut model, "@assets/s");

    assert!(model.file_picker_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let source_message = model.composer.source_message();
    assert_eq!(model.composer_text(), "[Image #1] ");
    assert_eq!(source_message.attachments().len(), 1);
    assert!(!model.file_picker_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let Some(AppEffect::SendConversationTurn {
        request,
        record_message_history,
    }) = effect
    else {
        panic!("expected conversation turn effect");
    };
    assert!(record_message_history.is_none());
    assert_eq!(
        request
            .transcript_user_message()
            .map(|message| message.attachments.len()),
        Some(1)
    );
}

#[test]
fn at_file_picker_enter_on_exact_image_path_inserts_attachment_instead_of_submitting() {
    let root = TempFileTree::new("enter-exact-image");
    root.write_file_with_content("assets/sample.png", valid_png_header());

    let mut model = file_picker_model(root.path());
    type_text(&mut model, "@assets/sample.png");

    assert!(model.file_picker_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let source_message = model.composer.source_message();
    assert!(effect.is_none());
    assert_eq!(model.composer_text(), "[Image #1] ");
    assert_eq!(source_message.attachments().len(), 1);
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
fn at_file_picker_enter_on_exact_visible_path_submits_prompt() {
    let root = TempFileTree::new("enter-exact-visible");
    root.write_file_with_content("src/lib.rs", b"pub fn demo() {}\n");

    let mut model = conversation_test_model();
    model.current_dir = root.path().display().to_string();
    type_text(&mut model, "@src/lib.rs");

    assert!(model.file_picker_active());
    assert!(
        model
            .current_file_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("lib.rs"))
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "@src/lib.rs");
}

#[test]
fn at_file_picker_enter_on_empty_results_does_not_send_composer() {
    let root = TempFileTree::new("enter-empty-results");
    root.write_file("src/lib.rs");

    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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
fn at_file_picker_enter_on_explicit_gitignored_file_submits_prompt() {
    let root = TempFileTree::new("enter-explicit-gitignored");
    root.write_file(".git/HEAD");
    root.write_file_with_content(".gitignore", b"target/\n");
    root.write_file_with_content("target/debug.log", b"hidden log\n");

    let mut model = conversation_test_model();
    model.current_dir = root.path().display().to_string();
    type_text(&mut model, "@target/debug.log");

    assert!(model.file_picker_active());
    assert!(
        model
            .current_file_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("No files"))
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "@target/debug.log");
}

#[test]
fn at_file_picker_enter_on_explicit_absolute_file_submits_prompt() {
    let root = TempFileTree::new("enter-explicit-absolute");
    root.write_file(".git/HEAD");
    root.write_file("src/lib.rs");

    let outside = TempFileTree::new("explicit-absolute-target");
    outside.write_file_with_content("outside.txt", b"outside text\n");
    let outside_path = outside.path().join("outside.txt");

    let mut model = conversation_test_model();
    model.current_dir = root.path().display().to_string();
    type_text(&mut model, &format!("@{}", outside_path.display()));

    assert!(model.file_picker_active());
    assert!(
        model
            .current_file_picker_render_result()
            .plain_lines
            .iter()
            .any(|line| line.contains("No files"))
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn { request, .. }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(
        request.message_text(),
        format!("@{}", outside_path.display())
    );
}

#[test]
fn file_picker_does_not_clear_status_lines_outside_popup_area() {
    let root = TempFileTree::new("keep-status-lines");
    root.write_file("src/lib.rs");

    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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

    let popup_buffer = render_model_buffer(&mut model, 40, 8);
    let popup_rows = rendered_rows(&popup_buffer);
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
    let restored_buffer = render_model_buffer(&mut model, 40, 8);
    let restored_rows = rendered_rows(&restored_buffer);

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

    let popup_buffer = render_model_buffer(&mut model, 40, 8);
    let popup_rows = rendered_rows(&popup_buffer);
    assert!(
        popup_rows.iter().any(|line| line.contains("src/lib.rs")),
        "first frame should render the flipped file picker popup: {popup_rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.file_picker_active());
    let restored_buffer = render_model_buffer(&mut model, 40, 8);
    let restored_rows = rendered_rows(&restored_buffer);

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
        StartupBannerOptions::default(),
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
        StartupBannerOptions::default(),
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
fn runtime_terminal_updates_keep_manual_scrollback() {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.transcript_mut().set_gap(0);
    for index in 0..16 {
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("history line {index}"));
    }
    let item_index = model.append_runtime_tool_activity_from_runtime(RuntimeToolActivity {
        activity_id: "call-terminal".to_string(),
        title: "Shell: seq 1000".to_string(),
        kind: runtime_domain::session::RuntimeToolKind::Execute,
        status: runtime_domain::session::RuntimeToolActivityStatus::InProgress,
        content: vec![
            runtime_domain::session::RuntimeToolActivityContent::Terminal {
                terminal_id: "call-terminal".to_string(),
            },
        ],
        locations: Vec::new(),
        raw_input: None,
        raw_output: None,
    });
    assert!(
        model.apply_runtime_terminal_snapshot_from_runtime(RuntimeTerminalSnapshot {
            terminal_id: "call-terminal".to_string(),
            command: Some("seq 1000".to_string()),
            cwd: None,
            output: (1..=40)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            truncated: false,
            exit_status: None,
            released: false,
        })
    );
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model.sync_document_viewport_to_bottom();

    let bottom_offset = model.document_runtime.viewport_y;
    model.scroll_document_by(-Model::document_mouse_wheel_delta());
    let scrolled_offset = model.document_runtime.viewport_y;
    assert!(
        scrolled_offset < bottom_offset,
        "fixture should manually scroll away from the active command tail"
    );
    assert!(model.document_runtime.manual_scroll);

    assert!(
        model.apply_runtime_terminal_snapshot_from_runtime(RuntimeTerminalSnapshot {
            terminal_id: "call-terminal".to_string(),
            command: Some("seq 1000".to_string()),
            cwd: None,
            output: (1..=80)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            truncated: false,
            exit_status: None,
            released: false,
        })
    );

    let layout = model.build_document_layout();
    let refreshed_bottom_offset = model.document_bottom_offset(layout.line_count());
    assert!(
        model.document_runtime.viewport_y < refreshed_bottom_offset,
        "terminal output refresh should not pull a manually scrolled viewport back to bottom"
    );
    assert!(
        model.document_runtime.manual_scroll,
        "terminal output refresh should keep manual scroll mode"
    );
    assert!(
        !model.document_runtime.follow_bottom,
        "terminal output refresh should not re-enable bottom-follow while user is reading scrollback"
    );

    assert!(
        model.update_runtime_tool_activity_from_runtime(
            item_index,
            RuntimeToolActivityUpdate {
                activity_id: "call-terminal".to_string(),
                status: Some(runtime_domain::session::RuntimeToolActivityStatus::Completed),
                raw_output: Some(
                    (1..=80)
                        .map(|line| format!("line {line}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .into(),
                ),
                ..RuntimeToolActivityUpdate::default()
            },
        )
    );
    let final_layout = model.build_document_layout();
    let final_bottom_offset = model.document_bottom_offset(final_layout.line_count());
    assert!(
        model.document_runtime.viewport_y < final_bottom_offset,
        "final tool update should not pull a manually scrolled viewport back to bottom"
    );
    assert!(
        model.document_runtime.manual_scroll,
        "final tool update should keep manual scroll mode"
    );
}

#[test]
fn height_only_resize_keeps_transcript_render_stable() {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
        let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), style_mode);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let forced_stale_index = crate::transcript::TranscriptItemMetricsIndex {
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
        let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Cx);
    model.set_window(80, 24);
    model.set_palette(default_palette(), true);
    model
        .composer_mut()
        .reset_text_and_move_to_end("中英 mixed long composer text ".repeat(120));
    model.sync_composer_height();
    let _ = model.build_document_layout();

    crate::composer::reset_render_document_call_count();
    model.composer_mut().move_to_begin();
    model.sync_document_viewport_for_composer_cursor();
    let _ = model.build_document_layout();

    assert_eq!(
        crate::composer::render_document_call_count(),
        0,
        "cursor-only layout refresh should reuse the cached long composer document"
    );
}

#[test]
fn sync_transcript_render_does_not_schedule_idle_history_refinement() {
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
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
    let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);

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
            let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                continue;
            }

            let index = model.exactize_visible_transcript_window_until_stable(index);
            let Some((next_start, next_count)) =
                model.current_visible_transcript_window_for_index(&index)
            else {
                continue;
            };
            let next_overscan_lines = crate::transcript::viewport_overscan_line_budget(next_count);
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
    let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);
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
fn runtime_permission_accept_key_preserves_target_identity() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: target.clone(),
        request: RuntimePermissionRequest::new(
            "conversation-permission-1",
            Some("Write TEMP.md".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "allow_once",
                    "Yes",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject_once",
                    "No",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
    });

    assert!(model.tool_approval_panel_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('y'))));

    assert_eq!(
        effect,
        Some(AppEffect::RespondRuntimePermission {
            target,
            request_id: "conversation-permission-1".to_string(),
            option_id: Some("allow_once".to_string()),
        })
    );
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn runtime_permission_reject_key_preserves_target_identity() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: target.clone(),
        request: RuntimePermissionRequest::new(
            "conversation-permission-2",
            Some("Edit TEMP.md".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "allow_once",
                    "Yes",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject_once",
                    "No",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('n'))));

    assert_eq!(
        effect,
        Some(AppEffect::RespondRuntimePermission {
            target,
            request_id: "conversation-permission-2".to_string(),
            option_id: Some("reject_once".to_string()),
        })
    );
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn runtime_bash_permission_panel_uses_description_and_hides_waiting_tool_row() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.apply_runtime_event(RuntimeEvent::ToolActivityStarted {
        target: target.clone(),
        activity: RuntimeToolActivity {
            activity_id: "call-bash".to_string(),
            title: "Shell: echo hi".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::InProgress,
            content: vec![RuntimeToolActivityContent::Terminal {
                terminal_id: "call-bash".to_string(),
            }],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "command": "echo hi" }).into()),
            raw_output: None,
        },
    });
    assert!(
        model
            .transcript_plain_items()
            .join("\n")
            .contains("Waiting..."),
        "execute tool row should normally show a waiting detail before approval is shown"
    );

    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target,
        request: RuntimePermissionRequest::new(
            "conversation-permission-1",
            Some("Shell: echo hi".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "allow_once",
                    "Yes",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject_once",
                    "No",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        )
        .with_tool_activity(RuntimeToolActivityUpdate {
            activity_id: "call-bash".to_string(),
            title: Some("Shell: echo hi".to_string()),
            kind: Some(RuntimeToolKind::Execute),
            status: Some(RuntimeToolActivityStatus::Pending),
            content: Some(vec![RuntimeToolActivityContent::Text(
                "Requesting approval to run echo hi".to_string(),
            )]),
            locations: Some(Vec::new()),
            raw_input: Some(
                serde_json::json!({
                    "command": "echo hi",
                    "description": "Check whether shell execution is available",
                    "workdir": "crates/terminal-ui",
                    "timeout": 5
                })
                .into(),
            ),
            raw_output: None,
        }),
    });

    assert!(model.tool_approval_panel_active());
    assert_eq!(model.tool_approval_panel.title, "echo hi");
    assert!(
        model.tool_approval_panel.details.iter().any(|detail| {
            detail.label == "Reason" && detail.value == "Check whether shell execution is available"
        }),
        "approval panel should show the model-provided command description under a Reason label: {:?}",
        model.tool_approval_panel.details
    );
    assert!(
        model
            .tool_approval_panel
            .details
            .iter()
            .any(|detail| { detail.label == "Workdir" && detail.value == "crates/terminal-ui" }),
        "approval panel should surface the command workdir when provided: {:?}",
        model.tool_approval_panel.details
    );
    assert!(
        !model
            .transcript_plain_items()
            .join("\n")
            .contains("echo hi"),
        "transcript should not duplicate the pending command row while the approval panel is open"
    );
}

#[test]
fn runtime_bash_permission_panel_omits_missing_description() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivityContent,
        RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target,
        request: RuntimePermissionRequest::new(
            "conversation-permission-1",
            Some("Shell: echo hi".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "allow_once",
                    "Yes",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject_once",
                    "No",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        )
        .with_tool_activity(RuntimeToolActivityUpdate {
            activity_id: "call-bash".to_string(),
            title: Some("Shell: echo hi".to_string()),
            kind: Some(RuntimeToolKind::Execute),
            status: Some(RuntimeToolActivityStatus::Pending),
            content: Some(vec![RuntimeToolActivityContent::Text(
                "Requesting approval to run echo hi".to_string(),
            )]),
            raw_input: Some(
                serde_json::json!({
                    "command": "echo hi",
                    "description": "   ",
                    "timeout": 5
                })
                .into(),
            ),
            ..RuntimeToolActivityUpdate::default()
        }),
    });

    assert!(model.tool_approval_panel_active());
    assert!(
        model
            .tool_approval_panel
            .details
            .iter()
            .all(|detail| detail.label != "Reason"),
        "approval panel should omit missing or blank descriptions: {:?}",
        model.tool_approval_panel.details
    );
    assert!(
        model
            .tool_approval_panel
            .details
            .iter()
            .all(|detail| detail.value != "Not provided"),
        "approval panel should not synthesize a Not provided placeholder: {:?}",
        model.tool_approval_panel.details
    );
}

#[test]
fn runtime_permission_panel_closes_when_runtime_retries_or_fails() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: target.clone(),
        request: RuntimePermissionRequest::new(
            "conversation-permission-3",
            Some("Write TEMP.md".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "allow_once",
                    "Yes",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject_once",
                    "No",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
    });
    assert!(model.tool_approval_panel_active());

    model.apply_runtime_event(RuntimeEvent::Retrying {
        target: Some(target.clone()),
        message: "Reconnecting... 1/2".to_string(),
    });
    assert!(!model.tool_approval_panel_active());

    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: target.clone(),
        request: RuntimePermissionRequest::new(
            "conversation-permission-4",
            Some("Edit TEMP.md".to_string()),
            vec![RuntimePermissionOption::new(
                "reject_once",
                "No",
                RuntimePermissionOptionKind::RejectOnce,
            )],
        ),
    });
    assert!(model.tool_approval_panel_active());

    model.apply_runtime_event(RuntimeEvent::Failed {
        target: Some(target),
        message: "request failed".to_string(),
    });
    assert!(!model.tool_approval_panel_active());
}

#[test]
fn stream_activity_line_uses_dynamic_codex_style_indicator() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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
        StartupBannerOptions::default(),
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
        StartupBannerOptions::default(),
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
fn esc_interrupts_conversation_after_configured_press_count() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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
fn esc_interrupts_conversation_immediately_when_configured_for_one_press() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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
fn enter_during_conversation_activity_does_not_append_unsent_message() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
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
    assert_eq!(model.current_status_notice_text(), "");
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Chat request is already running")
    );
    assert_eq!(model.composer_text(), "second message");
    assert!(model.transcript_plain_items().is_empty());
}

#[test]
fn stream_activity_line_shows_interrupt_hint() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.show_stream_activity("Kimi Code CLI");
    let line = model.current_stream_activity_render_result().plain_line;

    assert!(line.contains("esc 2x to interrupt"));
}

#[test]
fn esc_interrupts_stream_activity_after_configured_press_count() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 2,
            ..ModelOptions::default()
        },
    );
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
        StartupBannerOptions::default(),
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
        StartupBannerOptions::default(),
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
    let mut model = Model::new_with_style_mode(StartupBannerOptions::default(), StyleMode::Ms);
    model.transcript_mut().clear();
    model.current_dir = root.display().to_string();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model
}

fn skill_picker_model() -> Model {
    skill_picker_model_with_manual_skills(
        vec![
            PromptAssemblyDiscoveredSkill {
                skill_name: "code-review".to_string(),
                title: "Code Review".to_string(),
                description: "Review code and inspect regressions".to_string(),
                origin: PromptSourceOrigin::Project,
                selection_scope: PromptAssemblyScope::Project,
                skill_path: "/tmp/code-review/SKILL.md".to_string(),
                body: "# Code Review".to_string(),
                can_select_for_discovery: true,
                selected: false,
                selected_order: None,
            },
            PromptAssemblyDiscoveredSkill {
                skill_name: "brainstorming".to_string(),
                title: "Brainstorming".to_string(),
                description: "Explore intent, requirements, and design before implementation"
                    .to_string(),
                origin: PromptSourceOrigin::Builtin,
                selection_scope: PromptAssemblyScope::Project,
                skill_path: "/tmp/brainstorming/SKILL.md".to_string(),
                body: "# Brainstorming".to_string(),
                can_select_for_discovery: true,
                selected: false,
                selected_order: None,
            },
        ],
        8,
    )
}

fn overflowing_skill_picker_model() -> Model {
    let skills = (0..10)
        .map(|index| PromptAssemblyDiscoveredSkill {
            skill_name: format!("skill-{index:02}"),
            title: format!("Skill {index:02}"),
            description: format!("Skill description {index:02}"),
            origin: PromptSourceOrigin::Project,
            selection_scope: PromptAssemblyScope::Project,
            skill_path: format!("/tmp/skill-{index:02}/SKILL.md"),
            body: format!("# Skill {index:02}"),
            can_select_for_discovery: true,
            selected: false,
            selected_order: None,
        })
        .collect();
    skill_picker_model_with_manual_skills(skills, 5)
}

fn skill_picker_model_with_manual_skills(
    manual_skills: Vec<PromptAssemblyDiscoveredSkill>,
    file_picker_popup_height: u16,
) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            file_picker_popup_height,
            prompt_assembly: Some(PromptAssemblyManagerSnapshot {
                snapshot: resolve_prompt_assembly(&PromptAssemblyInput::default()),
                prelude: PromptPreludeSnapshot::default(),
                managed_sources: Vec::new(),
                sources: Vec::new(),
                extra_prompt_candidates: Vec::new(),
                discovered_skills: Vec::new(),
                manual_skills,
                tool_candidates: Vec::new(),
                dynamic_environment_candidates: Vec::new(),
                diagnostics: Vec::new(),
                builtin_core_system_body: String::new(),
                global_core_system_override: None,
                project_core_system_override: None,
            }),
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model
}

fn custom_prompt_picker_model() -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            file_picker_popup_height: 8,
            prompt_assembly: Some(PromptAssemblyManagerSnapshot {
                snapshot: resolve_prompt_assembly(&PromptAssemblyInput::default()),
                prelude: PromptPreludeSnapshot::default(),
                managed_sources: Vec::new(),
                sources: Vec::new(),
                extra_prompt_candidates: vec![
                    PromptAssemblyExtraPromptCandidate {
                        reference_id: "review-rules".to_string(),
                        title: "Review Rules".to_string(),
                        origin: PromptSourceOrigin::Project,
                        body: "Review every diff for regressions and missing tests.".to_string(),
                        selected: false,
                    },
                    PromptAssemblyExtraPromptCandidate {
                        reference_id: "release-checklist".to_string(),
                        title: "Release Checklist".to_string(),
                        origin: PromptSourceOrigin::Global,
                        body: "Verify changelog, version, and rollout notes.".to_string(),
                        selected: false,
                    },
                ],
                discovered_skills: Vec::new(),
                manual_skills: Vec::new(),
                tool_candidates: Vec::new(),
                dynamic_environment_candidates: Vec::new(),
                diagnostics: Vec::new(),
                builtin_core_system_body: String::new(),
                global_core_system_override: None,
                project_core_system_override: None,
            }),
            ..ModelOptions::default()
        },
    );
    model.transcript_mut().clear();
    model.set_window(40, 8);
    model.set_palette(default_palette(), true);
    model
}

fn file_picker_test_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
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
    let buffer = render_model_buffer(model, width, height);
    rendered_rows(&buffer)
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
            std::env::temp_dir().join(format!("hunea-file-picker-{name}-{}", std::process::id()));
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
}

impl Drop for TempFileTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn valid_png_header() -> &'static [u8] {
    &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
}
