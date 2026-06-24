use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{AppEvent, Model, StartupBannerOptions};

use super::common::{diverse_rows, ready_picker_model, selection_stability_rows};

#[test]
fn slash_enters_search_without_typing_in_composer() {
    let mut model = ready_picker_model();
    assert_eq!(model.composer_text(), "");
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_searching());
    assert!(state.search_query().is_empty());
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
    assert_eq!(state.filtered_count(), 2);
    assert_eq!(
        state.selected_row().map(|r| r.text.as_str()),
        Some("GIT diff")
    );
}

#[test]
fn search_mode_treats_lowercase_c_as_query_text_not_copy_command() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, diverse_rows());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('a'))));

    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(effect, None);
    assert_eq!(state.search_query(), "ca");
    assert_eq!(state.filtered_count(), 1);
    assert_eq!(
        state.selected_row().map(|row| row.text.as_str()),
        Some("cargo test")
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
    assert!(!state.has_filtered_rows());
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
    assert!(!state.is_searching());
    assert!(state.search_query().is_empty());
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
    assert!(state.is_searching());
    assert_eq!(state.search_query(), "n");
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('u'),
        KeyModifiers::CONTROL,
    )));
    let state = model.message_history_picker.as_ref().unwrap();
    assert!(state.is_searching());
    assert!(state.search_query().is_empty());
    assert_eq!(state.filtered_count(), 2);
}

#[test]
fn search_hjkl_are_query_text_not_navigation() {
    let mut model = ready_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    for ch in ['h', 'j', 'k', 'l'] {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.search_query(), "hjkl");
    assert_eq!(state.filtered_count(), 0);
    assert_eq!(state.selected_visible_position(), None);
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
fn filter_preserves_selected_row_when_matching_position_changes() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 24);
    let request_id = model.open_message_history_picker_loading_at(10_000);
    model.apply_message_history_picker_rows(request_id, selection_stability_rows());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    assert_eq!(
        model
            .message_history_picker
            .as_ref()
            .unwrap()
            .selected_row()
            .map(|row| row.id),
        Some(20)
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    for ch in "target".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }

    let state = model.message_history_picker.as_ref().unwrap();
    assert_eq!(state.filtered_indices_for_test(), &[1, 2]);
    assert_eq!(state.selected_row().map(|row| row.id), Some(20));
}
