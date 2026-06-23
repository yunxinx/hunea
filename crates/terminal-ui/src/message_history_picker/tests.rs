use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_store::MessageHistoryRow;

use crate::{AppEffect, AppEvent, Model, StartupBannerOptions, modal_layer::ModalLayer};

fn ctrl_r() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
}

fn type_text(model: &mut Model, text: &str) {
    for ch in text.chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
}

fn sample_rows() -> Vec<MessageHistoryRow> {
    vec![
        MessageHistoryRow {
            id: 1,
            ts: 1_000,
            text: "older prompt".to_string(),
        },
        MessageHistoryRow {
            id: 2,
            ts: 2_000,
            text: "newest prompt".to_string(),
        },
    ]
}

fn ready_picker_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(sample_rows());
    model
}

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
fn apply_rows_selects_newest_row() {
    let model = ready_picker_model();
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.selected, 1);
    assert_eq!(state.rows[state.selected].text.as_str(), "newest prompt");
}

#[test]
fn empty_history_shows_empty_state_and_esc_closes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("composer unchanged".to_string());
    model.open_message_history_picker_loading_at(5_000);
    model.apply_message_history_picker_rows(vec![]);

    assert!(model.message_history_picker_active());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "composer unchanged");
}

#[test]
fn enter_is_swallowed_without_closing_picker() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert!(model.message_history_picker_active());
}

#[test]
fn up_moves_from_newest_to_older_row() {
    let mut model = ready_picker_model();
    assert_eq!(model.message_history_picker.as_ref().unwrap().selected, 1);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(model.message_history_picker.as_ref().unwrap().selected, 0);
}

#[test]
fn closing_picker_clears_transient_state() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert_eq!(model.message_history_picker, None);
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
fn noop_coordinator_loads_empty_rows_on_open_effect() {
    use crate::runner::{NoopRuntimeCoordinator, run_open_message_history_picker_effect};

    let mut coordinator = NoopRuntimeCoordinator;
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    run_open_message_history_picker_effect(&mut model, &mut coordinator);
    assert!(model.message_history_picker_active());
    assert!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(!model.message_history_picker.as_ref().unwrap().is_loading);
}
