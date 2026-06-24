use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{AppEffect, AppEvent, Model, StartupBannerOptions};

use super::common::{long_message_for_copy, ready_picker_model};

#[test]
fn space_opens_preview_active_flag() {
    let mut model = ready_picker_model();
    assert!(!model.message_history_picker_preview_active());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.message_history_picker_preview_active());
    assert!(model.message_history_picker_active());
}

#[test]
fn esc_closes_preview_before_picker() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.message_history_picker_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(model.message_history_picker_active());
    assert!(!model.message_history_picker_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.message_history_picker_active());
}

#[test]
fn c_on_list_copies_full_selected_text() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("older prompt".to_string()))
    );
}

#[test]
fn c_in_preview_copies_full_text_not_truncated() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(40, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, long_message_for_copy());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "short in list but this is the full message body for clipboard".to_string()
        ))
    );
}

#[test]
fn shift_c_does_not_copy_in_message_history_picker() {
    let mut model = ready_picker_model();
    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )));
    assert_eq!(effect, None);
}

#[test]
fn copy_does_not_change_composer_or_leaf() {
    let mut model = ready_picker_model();
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("unchanged".to_string());
    let _ = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    assert_eq!(model.composer_text(), "unchanged");
}
