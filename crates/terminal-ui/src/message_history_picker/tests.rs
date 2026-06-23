use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use runtime_domain::session::MessageHistoryRow;

use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions, modal_layer::ModalLayer,
    overlay_input_result::OverlayInputResult,
};

#[cfg(test)]
mod render;

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

fn diverse_rows() -> Vec<MessageHistoryRow> {
    vec![
        MessageHistoryRow {
            id: 1,
            ts: 1_000,
            text: "git status".to_string(),
        },
        MessageHistoryRow {
            id: 2,
            ts: 2_000,
            text: "cargo test".to_string(),
        },
        MessageHistoryRow {
            id: 3,
            ts: 3_000,
            text: "GIT diff".to_string(),
        },
    ]
}

fn ready_picker_model() -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, sample_rows());
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
    let request_id = model.open_message_history_picker_loading_at(5_000);
    model.apply_message_history_picker_rows(request_id, vec![]);

    assert!(model.message_history_picker_active());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "composer unchanged");
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
    assert_eq!(model.message_history_picker.as_ref().unwrap().selected, 0);

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
    assert_eq!(state.rows.len(), 2);
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

fn long_message_for_copy() -> Vec<MessageHistoryRow> {
    vec![MessageHistoryRow {
        id: 1,
        ts: 1_000,
        text: "short in list but this is the full message body for clipboard".to_string(),
    }]
}

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

#[test]
fn slash_enters_search_without_typing_in_composer() {
    let mut model = ready_picker_model();
    assert_eq!(model.composer_text(), "");
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_searching);
    assert!(state.search_query.is_empty());
}

#[test]
fn search_filters_case_insensitive_on_full_text() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, diverse_rows());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('g'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('i'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('t'))));
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.filtered_indices.len(), 2);
    assert_eq!(
        state.selected_row().map(|r| r.text.as_str()),
        Some("GIT diff")
    );
}

#[test]
fn search_no_match_shows_empty_filter_state() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('z'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('z'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('z'))));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.filtered_indices.is_empty());
}

#[test]
fn esc_exits_search_then_closes_picker() {
    let mut model = ready_picker_model();
    model
        .composer_mut()
        .replace_text_and_move_to_end_for_edit("composer unchanged".to_string());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('o'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(model.message_history_picker_active());
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(!state.is_searching);
    assert!(state.search_query.is_empty());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.message_history_picker_active());
    assert_eq!(model.composer_text(), "composer unchanged");
}

#[test]
fn backspace_and_ctrl_u_in_search_mode() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('n'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('e'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_searching);
    assert_eq!(state.search_query, "n");
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    )));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_searching);
    assert!(state.search_query.is_empty());
    assert_eq!(state.filtered_indices.len(), 2);
}

#[test]
fn search_hjkl_are_query_text_not_navigation() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    for ch in ['h', 'j', 'k', 'l'] {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.search_query, "hjkl");
    assert_eq!(state.filtered_indices.len(), 0);
    assert_eq!(state.selected, 0);
}

#[test]
fn filter_preserves_selected_row_when_still_visible() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, diverse_rows());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_row_index(),
        Some(1)
    );
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('a'))));
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.selected_row_index(), Some(1));
    assert_eq!(
        state.selected_row().map(|r| r.text.as_str()),
        Some("cargo test")
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
    assert_eq!(model.message_history_picker.as_ref().unwrap().selected, 1);
    let _ = model.handle_message_history_picker_mouse_down(MouseButton::Left, 4, 2);
    assert_eq!(model.message_history_picker.as_ref().unwrap().selected, 0);
    assert_eq!(
        model.handle_message_history_picker_mouse_down(MouseButton::Right, 4, 2),
        OverlayInputResult::Handled
    );
}

#[test]
fn noop_coordinator_keeps_picker_loading_until_runtime_rows_event() {
    use crate::runner::{NoopRuntimeCoordinator, run_open_message_history_picker_effect};

    let mut coordinator = NoopRuntimeCoordinator;
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    run_open_message_history_picker_effect(&mut model, &mut coordinator);
    assert!(model.message_history_picker_active());
    assert!(model.message_history_picker.as_ref().unwrap().is_loading);
    assert!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .rows
            .is_empty()
    );
}
