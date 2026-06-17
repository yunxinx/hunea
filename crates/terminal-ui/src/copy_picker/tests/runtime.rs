use super::*;

#[test]
fn late_entry_tree_payload_is_ignored_without_entry_tree_intent() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.copy_picker_active());
    assert!(!model.entry_tree_active());

    model.apply_runtime_event(RuntimeEvent::SessionTreeLoaded {
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "late-user",
                SessionTreeRowKind::User,
                "late user",
                Some("late user".to_string()),
                Some("late-user"),
            )],
            current_row_id: Some("late-user".to_string()),
        },
    });

    assert!(
        !model.entry_tree_active(),
        "late entry-tree payload must not open an overlay without an active entry-tree intent"
    );
}

#[test]
fn late_copy_picker_payload_is_ignored_after_copy_picker_closes() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.copy_picker_active());
    assert!(!model.entry_tree_active());

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoaded {
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "late-user",
                SessionTreeRowKind::User,
                "late user",
                Some("late user".to_string()),
                Some("late-user"),
            )],
            current_row_id: Some("late-user".to_string()),
        },
    });

    assert!(
        !model.copy_picker_active(),
        "late copy-picker payload must be ignored after the copy picker closes"
    );
    assert!(
        !model.entry_tree_active(),
        "late copy-picker payload must not be routed into entry tree"
    );
}

#[test]
fn direct_copy_picker_payload_is_ignored_without_active_picker() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row(
            "late-user",
            SessionTreeRowKind::User,
            "late user",
            Some("late user".to_string()),
            Some("late-user"),
        )],
        current_row_id: Some("late-user".to_string()),
    });

    assert!(
        !model.copy_picker_active(),
        "copy picker payload application should not create an overlay without an active intent"
    );
}

#[test]
fn direct_copy_picker_error_is_ignored_without_active_picker() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.show_copy_picker_error("copy picker failed after close");

    assert!(
        !model.copy_picker_active(),
        "copy picker errors should not create an overlay without an active intent"
    );
}

#[test]
fn copy_picker_load_failed_event_renders_overlay_error_without_toast() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoadFailed {
        message: "session tree index is corrupt".to_string(),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows.iter()
            .any(|row| row.contains("session tree index is corrupt")),
        "copy picker async load errors should render inside the overlay: {rows:?}"
    );
    assert_eq!(model.active_toast_text_for_test(), None);
}
