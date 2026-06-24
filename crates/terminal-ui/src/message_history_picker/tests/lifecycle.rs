use crossterm::event::{KeyCode, KeyEvent};

use crate::{AppEffect, AppEvent, Model, StartupBannerOptions, modal_layer::ModalLayer};

use super::common::{ctrl_r, ready_picker_model, sample_rows, type_text};

#[test]
fn ctrl_r_opens_picker_preserves_composer() {
    let mut model = Model::new(StartupBannerOptions::default());
    type_text(&mut model, "draft kept");

    let effect = model.update(AppEvent::Key(ctrl_r()));

    assert_eq!(effect, Some(AppEffect::OpenMessageHistory));
    assert_eq!(model.composer_text(), "draft kept");
}

#[test]
fn ctrl_r_blocked_when_command_panel_active() {
    let mut model = Model::new(StartupBannerOptions::default());
    type_text(&mut model, "/res");

    let effect = model.update(AppEvent::Key(ctrl_r()));

    assert_eq!(effect, None);
    assert!(!model.message_history_picker_active());
}

#[test]
fn ctrl_r_blocked_when_file_picker_active() {
    let mut model = Model::new(StartupBannerOptions::default());
    type_text(&mut model, "@");

    let effect = model.update(AppEvent::Key(ctrl_r()));

    assert_eq!(effect, None);
}

#[test]
fn resend_command_clears_composer_and_opens_picker_effect() {
    let mut model = Model::new(StartupBannerOptions::default());
    type_text(&mut model, "/resend");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, Some(AppEffect::OpenMessageHistory));
    assert_eq!(model.composer_text(), "");
}

#[test]
fn empty_history_shows_empty_state_and_esc_closes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("composer unchanged".to_string());
    let request_id = model.open_message_history_picker_loading_at(5_000);
    model.apply_message_history_picker_rows(request_id, vec![]);

    assert!(model.message_history_picker_active());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "composer unchanged");
}

#[test]
fn closing_picker_clears_transient_state() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(model.message_history_picker, None);
}

#[test]
fn late_rows_after_close_do_not_reopen_picker() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    model.apply_message_history_picker_rows(request_id, sample_rows());

    assert!(!model.message_history_picker_active());
}

#[test]
fn stale_rows_from_previous_open_do_not_replace_current_picker() {
    let mut model = Model::new(StartupBannerOptions::default());
    let stale_request_id = model.open_message_history_picker_loading_at(10_000);
    let current_request_id = model.open_message_history_picker_loading_at(11_000);

    model.apply_message_history_picker_rows(stale_request_id, sample_rows());

    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_loading);
    assert_eq!(
        model.message_history_picker_pending_request_id_for_test(),
        Some(current_request_id)
    );

    model.apply_message_history_picker_rows(current_request_id, sample_rows());

    let state = model.message_history_picker.as_ref().unwrap();
    assert!(!state.is_loading);
    assert_eq!(state.filtered_count(), 2);
}

#[test]
fn late_error_after_close_does_not_reopen_picker() {
    let mut model = Model::new(StartupBannerOptions::default());
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    model.show_message_history_picker_error(request_id, "load failed");

    assert!(!model.message_history_picker_active());
}

#[test]
fn modal_layer_message_history_is_lowest_priority() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.open_message_history_picker_loading();
    model.open_entry_tree_loading();
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::EntryTree));
    model.entry_tree = None;
    assert_eq!(model.top_modal_layer(), Some(ModalLayer::MessageHistory));
}

#[test]
fn noop_coordinator_reports_picker_unavailable_in_overlay() {
    use crate::runner::{NoopRuntimeCoordinator, run_open_message_history_picker_effect};

    let mut coordinator = NoopRuntimeCoordinator;
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    run_open_message_history_picker_effect(&mut model, &mut coordinator);
    assert!(model.message_history_picker_active());
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(!state.is_loading);
    assert_eq!(state.error.as_deref(), Some("Runtime is not available"));
    assert!(!state.has_rows());
}
