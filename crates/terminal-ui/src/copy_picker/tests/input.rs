use super::*;

#[test]
fn copy_command_opens_copy_picker_effect_and_clears_composer() {
    let mut model = Model::new(StartupBannerOptions::default());
    type_text(&mut model, "/copy");

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, Some(AppEffect::OpenCopyPicker));
    assert_eq!(model.composer_text(), "");
}

#[test]
fn copy_picker_enter_is_noop_and_tab_never_opens_branch_picker() {
    let mut model = ready_copy_picker_model();

    assert_eq!(
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter))),
        None
    );
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    assert!(model.copy_picker_active());
    assert!(!model.entry_tree_branch_picker_active());
}

#[test]
fn copy_picker_shift_c_copies_raw_text() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT,
    )));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("assistant raw".to_string()))
    );
}

#[test]
fn copy_picker_shift_a_selects_all_like_uppercase_a() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('A'),
        KeyModifiers::SHIFT,
    )));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "first user\n\n\nassistant raw\n\n\nsecond user".to_string()
        ))
    );
}

#[test]
fn copy_picker_mouse_click_moves_cursor_without_copying() {
    let mut model = ready_copy_picker_model();

    let effect = model.update(AppEvent::MouseDown {
        button: crossterm::event::MouseButton::Left,
        column: 4,
        row: 2,
    });
    let copied = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(effect, None);
    assert_eq!(
        copied,
        Some(AppEffect::CopySelection("first user".to_string()))
    );
}
