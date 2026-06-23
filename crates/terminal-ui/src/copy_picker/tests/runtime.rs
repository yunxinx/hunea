use super::*;

#[test]
fn late_entry_tree_payload_is_ignored_without_entry_tree_intent() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.copy_picker_active());
    assert!(!model.entry_tree_active());

    model.apply_runtime_event(RuntimeEvent::SessionTreeLoaded {
        request_id: SessionLoadRequestId::new(1),
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
        request_id: SessionLoadRequestId::new(1),
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
fn late_loaded_copy_picker_payload_is_ignored_after_initial_load_finishes() {
    let mut model = ready_copy_picker_model();

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoaded {
        request_id: SessionLoadRequestId::new(1),
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

    assert_eq!(
        model.copy_picker_row_ids_for_test(),
        vec!["user-1", "assistant-1", "user-2"],
        "a duplicate or late copy-picker payload must not replace the already interactive picker"
    );
}

#[test]
fn stale_copy_picker_payload_is_ignored_after_picker_reopens_loading() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(80, 12);
    model.set_palette(default_palette(), true);

    let stale_request_id = model.open_copy_picker_loading();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    let current_request_id = model.open_copy_picker_loading();

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoaded {
        request_id: stale_request_id,
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "stale-user",
                SessionTreeRowKind::User,
                "stale user",
                Some("stale user".to_string()),
                Some("stale-user"),
            )],
            current_row_id: Some("stale-user".to_string()),
        },
    });

    assert!(model.copy_picker_loading());
    assert_eq!(model.copy_picker_row_ids_for_test(), Vec::<&str>::new());

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoaded {
        request_id: current_request_id,
        payload: SessionTreePayload {
            rows: vec![tree_row(
                "current-user",
                SessionTreeRowKind::User,
                "current user",
                Some("current user".to_string()),
                Some("current-user"),
            )],
            current_row_id: Some("current-user".to_string()),
        },
    });

    assert!(!model.copy_picker_loading());
    assert_eq!(model.copy_picker_row_ids_for_test(), vec!["current-user"]);
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
    let request_id = model.copy_picker_pending_request_id_for_test().unwrap();

    model.apply_runtime_event(RuntimeEvent::CopyPickerTreeLoadFailed {
        request_id,
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
