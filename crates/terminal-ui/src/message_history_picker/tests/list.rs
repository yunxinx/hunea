use crossterm::event::{KeyCode, KeyEvent, MouseButton};

use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions, overlay_input_result::OverlayInputResult,
};

use super::common::{diverse_rows, ready_picker_model};

#[test]
fn apply_rows_selects_newest_row() {
    let model = ready_picker_model();
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.selected_visible_position(), Some(1));
    assert_eq!(
        state.selected_row().map(|row| row.text.as_str()),
        Some("newest prompt")
    );
}

#[test]
fn enter_with_empty_composer_restores_selected_and_closes_picker() {
    let mut model = ready_picker_model();
    assert!(model.composer_text().is_empty());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "newest prompt");
    assert_eq!(
        model.blind_recall().last_history_text(),
        Some("newest prompt")
    );
}

#[test]
fn enter_with_whitespace_only_composer_does_not_record_history() {
    let mut model = ready_picker_model();
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("   \t  ".to_string());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "newest prompt");
}

#[test]
fn enter_with_nonempty_composer_records_draft_then_restores() {
    let mut model = ready_picker_model();
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("draft kept".to_string());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_visible_position(),
        Some(0)
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::RecordMessageHistory {
            text: "draft kept".to_string(),
        })
    );
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "older prompt");
    assert_eq!(
        model.blind_recall().last_history_text(),
        Some("older prompt")
    );
}

#[test]
fn enter_does_not_emit_rewind_or_session_effects() {
    let mut model = ready_picker_model();
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(!matches!(
        effect,
        Some(AppEffect::SelectEntryRewind { .. })
            | Some(AppEffect::OpenEntryRewind)
            | Some(AppEffect::SwitchBranch { .. })
    ));
}

#[test]
fn up_moves_from_newest_to_older_row() {
    let mut model = ready_picker_model();
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_visible_position(),
        Some(1)
    );
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_visible_position(),
        Some(0)
    );
}

#[test]
fn message_history_picker_mouse_down_selects_visible_row() {
    let mut inactive = Model::new(StartupBannerOptions::default());
    assert_eq!(
        inactive.handle_message_history_picker_mouse_down(MouseButton::Left, 4, 2),
        OverlayInputResult::Ignored
    );

    let mut model = ready_picker_model();
    model.set_window(80, 12);
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_visible_position(),
        Some(1)
    );
    let _ = model.handle_message_history_picker_mouse_down(MouseButton::Left, 4, 2);
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_visible_position(),
        Some(0)
    );
    assert_eq!(
        model.handle_message_history_picker_mouse_down(MouseButton::Right, 4, 2),
        OverlayInputResult::Handled
    );
}

#[test]
fn message_history_picker_exposes_render_state_without_leaking_list_internals() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, diverse_rows());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('a'))));

    let state = model
        .message_history_picker
        .as_ref()
        .expect("picker should stay open while filtering");
    assert_eq!(state.filtered_count(), 1);
    assert!(state.has_rows());
    assert!(state.has_filtered_rows());
    assert_eq!(state.selected_visible_position(), Some(0));
    assert!(state.is_selected_visible_position(0));
    assert!(!state.is_selected_visible_position(1));
}
