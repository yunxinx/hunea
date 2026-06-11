use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use runtime_domain::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use runtime_domain::provider::ProviderKind;

use crate::{
    AppEffect, AppEvent, EscRewindMode, Model, ModelOptions, Sender, StartupBannerOptions,
    theme::default_palette,
};
use ratatui::style::Modifier;

#[test]
fn conversation_message_revisit_prefills_composer_and_truncates_history() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc))),
        None
    );
    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc))),
        None
    );
    assert!(
        model.transcript_overlay_active(),
        "second Esc should open the message_revisit transcript overlay"
    );
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(2)
    );

    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter))),
        Some(AppEffect::TruncateConversation {
            retained_user_turns: 1,
        })
    );

    assert!(!model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "second question");
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![
            (Sender::User, "first question".to_string()),
            (Sender::Assistant, "first answer".to_string()),
        ]
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    let Some(AppEffect::SendConversationTurn { request }) = effect else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert_eq!(request.message_text(), "second question");
}

#[test]
fn entry_esc_rewind_mode_opens_entry_rewind_instead_of_coarse_overlay() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            esc_rewind_mode: EscRewindMode::Entry,
            ..ModelOptions::default()
        },
    );
    model.set_palette(default_palette(), true);
    model.set_window(48, 12);
    model
        .transcript_mut()
        .append_message(Sender::User, "first question");

    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc))),
        None
    );
    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc))),
        Some(AppEffect::OpenEntryRewind)
    );
    assert!(!model.transcript_overlay_active());
}

#[test]
fn conversation_message_revisit_prefill_is_not_composer_undo_history() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter))),
        Some(AppEffect::TruncateConversation {
            retained_user_turns: 1,
        })
    );
    assert_eq!(model.composer_text(), "second question");

    // message revisit 同时截断 transcript；composer-only undo 不能伪造“恢复旧草稿”的半截历史。
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(model.composer_text(), "second question");
    assert_eq!(
        model.transcript_mut().source_messages(),
        vec![
            (Sender::User, "first question".to_string()),
            (Sender::Assistant, "first answer".to_string()),
        ]
    );
}

#[test]
fn conversation_message_revisit_highlight_projects_cx_half_height_frame_to_solid_selection() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let buffer = render_model_buffer(&mut model, 40, 8);
    let rows = buffer_rows(&buffer);
    let message_row = rows
        .iter()
        .position(|row| row.contains("› second question"))
        .expect("selected message_revisit user message should be visible");
    let frame_top_row = message_row - 1;
    let frame_bottom_row = message_row + 1;
    let content_row = message_row as u16;
    let selected_frame_top_row = frame_top_row as u16;
    let selected_frame_bottom_row = frame_bottom_row as u16;
    let palette = default_palette();
    let surface = default_palette()
        .surface
        .expect("default palette should provide a surface color");

    assert_eq!(rows[frame_top_row], " ".repeat(40));
    assert_eq!(rows[frame_bottom_row], " ".repeat(40));
    for column in 0..40 {
        let top = &buffer[(column, selected_frame_top_row)];
        let content = &buffer[(column, content_row)];
        let bottom = &buffer[(column, selected_frame_bottom_row)];
        assert_eq!(top.fg, palette.main);
        assert_eq!(bottom.fg, palette.main);
        assert_eq!(top.bg, surface);
        assert_eq!(bottom.bg, surface);
        assert_eq!(content.fg, palette.main);
        assert_eq!(content.bg, surface);
        assert!(
            top.modifier.contains(Modifier::REVERSED),
            "message_revisit highlight must reverse the upper half frame row"
        );
        assert!(
            content.modifier.contains(Modifier::REVERSED),
            "message_revisit highlight must reverse the selected content row"
        );
        assert!(
            bottom.modifier.contains(Modifier::REVERSED),
            "message_revisit highlight must reverse the lower half frame row"
        );
    }
}

#[test]
fn conversation_message_revisit_overlay_steps_to_older_user_message() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Left)));
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(0)
    );
    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter))),
        Some(AppEffect::TruncateConversation {
            retained_user_turns: 0,
        })
    );

    assert!(!model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "first question");
    assert_eq!(
        model.transcript_mut().source_messages(),
        Vec::<(Sender, String)>::new()
    );
}

#[test]
fn conversation_message_revisit_overlay_esc_closes_without_selecting_older_message() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(2)
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "");
    assert_eq!(
        model.transcript_mut().source_messages(),
        two_turn_source_messages()
    );
}

#[test]
fn conversation_message_revisit_overlay_ignores_q_as_close_key() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))));

    assert!(
        model.transcript_overlay_active(),
        "q should not close the message revisit overlay"
    );
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(2),
        "q should not change the selected revisit target"
    );
}

#[test]
fn conversation_message_revisit_overlay_left_right_keys_step_between_user_messages() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Left)));
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(0)
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Right)));
    assert_eq!(
        model
            .transcript_overlay
            .as_ref()
            .and_then(|overlay| overlay.highlight_item_index),
        Some(2)
    );
}

#[test]
fn conversation_message_revisit_overlay_disables_mouse_capture_for_terminal_selection() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(
        !model.wants_mouse_capture(),
        "message_revisit overlay should match Ctrl+T by disabling mouse capture for terminal selection"
    );
}

#[test]
fn conversation_message_revisit_overlay_up_down_scroll_window_without_changing_selection() {
    let mut model = conversation_test_model();
    append_scrollable_turns(&mut model, 12);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let selected_item = model
        .transcript_overlay
        .as_ref()
        .and_then(|overlay| overlay.highlight_item_index);
    let before_offset = model
        .transcript_overlay
        .as_ref()
        .map(|overlay| overlay.scroll_offset)
        .unwrap_or_default();
    assert!(
        before_offset > 0,
        "fixture should open the selected latest user message near the bottom"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let overlay = model
        .transcript_overlay
        .as_ref()
        .expect("message_revisit overlay should stay open after scroll");
    assert!(
        overlay.scroll_offset < before_offset,
        "Up should scroll the transcript window so terminal alternate-scroll wheel events do not move selection"
    );
    assert_eq!(
        overlay.highlight_item_index, selected_item,
        "scroll keys must not change the selected message_revisit message"
    );
}

#[test]
fn conversation_message_revisit_notice_timeout_resets_armed_state() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(
        model.current_status_notice_text().contains("Esc again"),
        "first Esc should show the message_revisit hint"
    );

    let timeout = model
        .timeout_event(std::time::Instant::now() + std::time::Duration::from_secs(2))
        .expect("message_revisit notice should time out");
    assert_eq!(model.update(timeout), None);
    assert!(model.current_status_notice_text().is_empty());
    assert!(
        !model.message_revisit.is_armed,
        "message_revisit confirmation state should expire with its status notice"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.transcript_overlay_active());
    assert!(
        model.current_status_notice_text().contains("Esc again"),
        "Esc after the hint expires should start a fresh confirmation window"
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        two_turn_source_messages()
    );
}

#[test]
fn esc_canceling_exit_confirmation_does_not_arm_message_revisit() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.current_status_notice_text().contains("exit"));

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(model.current_status_notice_text().is_empty());
    assert!(!model.message_revisit.is_armed);
    assert!(!model.transcript_overlay_active());
    assert_eq!(
        model.transcript_mut().source_messages(),
        two_turn_source_messages()
    );
}

#[test]
fn conversation_message_revisit_does_not_start_when_composer_has_draft() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);
    model.composer_mut().insert_text("draft");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "draft");
    assert_eq!(
        model.transcript_mut().source_messages(),
        two_turn_source_messages()
    );
}

#[test]
fn ordinary_transcript_overlay_esc_still_closes_without_message_revisit() {
    let mut model = conversation_test_model();
    append_two_turns(&mut model);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.transcript_overlay_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "");
    assert_eq!(
        model.transcript_mut().source_messages(),
        two_turn_source_messages()
    );
}

fn conversation_test_model() -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: message_revisit_test_model_catalog(),
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

fn message_revisit_test_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}

fn append_two_turns(model: &mut Model) {
    model
        .transcript_mut()
        .append_message(Sender::User, "first question");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "first answer");
    model
        .transcript_mut()
        .append_message(Sender::User, "second question");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "second answer");
    model.sync_transcript_render();
}

fn append_scrollable_turns(model: &mut Model, turn_count: usize) {
    for index in 0..turn_count {
        model
            .transcript_mut()
            .append_message(Sender::User, format!("question {index}"));
        model
            .transcript_mut()
            .append_message(Sender::Assistant, format!("answer {index}"));
    }
    model.sync_transcript_render();
}

fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let area = ratatui::layout::Rect::new(0, 0, width, height);
    let mut buffer = ratatui::buffer::Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

fn buffer_rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            (0..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect()
}

fn two_turn_source_messages() -> Vec<(Sender, String)> {
    vec![
        (Sender::User, "first question".to_string()),
        (Sender::Assistant, "first answer".to_string()),
        (Sender::User, "second question".to_string()),
        (Sender::Assistant, "second answer".to_string()),
    ]
}
