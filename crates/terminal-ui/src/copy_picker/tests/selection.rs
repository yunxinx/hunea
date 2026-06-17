use super::*;

#[test]
fn copy_picker_filters_to_user_and_assistant_rows_and_copies_selection_in_list_order() {
    let mut model = ready_copy_picker_model();

    assert_eq!(
        model.copy_picker_row_ids_for_test(),
        vec!["user-1", "assistant-1", "user-2"]
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "first user\n\n\nsecond user".to_string()
        ))
    );
    assert!(model.copy_picker_active());
}

#[test]
fn copy_picker_raw_and_display_differ_for_assistant_with_tool_descendants() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let raw = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));
    let display = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('c'))));

    assert_eq!(
        raw,
        Some(AppEffect::CopySelection("assistant raw".to_string()))
    );
    assert_eq!(
        display,
        Some(AppEffect::CopySelection(
            "assistant display\n\nTool call `read_file` (call-1)".to_string()
        ))
    );
}

#[test]
fn copy_picker_select_all_then_invert_updates_copy_targets() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('A'))));
    let all = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('A'))));
    let inverted = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        all,
        Some(AppEffect::CopySelection(
            "first user\n\n\nassistant raw\n\n\nsecond user".to_string()
        ))
    );
    assert_eq!(
        inverted,
        Some(AppEffect::CopySelection("second user".to_string()))
    );
}

#[test]
fn copy_picker_inverts_partial_selection() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('A'))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection("assistant raw".to_string()))
    );
}

#[test]
fn copy_picker_remaps_selected_rows_by_identity_when_payload_refreshes() {
    let mut model = ready_copy_picker_model();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![
            tree_row(
                "assistant-1",
                SessionTreeRowKind::Assistant,
                "assistant raw refreshed",
                None,
                Some("assistant-1"),
            ),
            tree_row(
                "user-1",
                SessionTreeRowKind::User,
                "first user refreshed",
                Some("first user refreshed".to_string()),
                Some("user-1"),
            ),
            tree_row(
                "user-2",
                SessionTreeRowKind::User,
                "second user refreshed",
                Some("second user refreshed".to_string()),
                Some("user-2"),
            ),
        ],
        current_row_id: Some("user-1".to_string()),
    });

    assert_eq!(
        model.copy_picker_selected_row_indices_for_test(),
        vec![0, 2]
    );

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "assistant raw refreshed\n\n\nsecond user refreshed".to_string()
        ))
    );
}

#[test]
fn copy_picker_clipboard_failure_keeps_selection_for_retry() {
    let mut model = ready_copy_picker_model();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    model.update(AppEvent::SelectionCopyCompleted { success: false });
    let retry = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(
        model.active_toast_text_for_test(),
        Some("Copy selection failed")
    );
    assert_eq!(
        retry,
        Some(AppEffect::CopySelection(
            "first user\n\n\nsecond user".to_string()
        ))
    );
}

#[test]
fn copy_picker_copy_attempt_on_empty_list_shows_result_toast() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(60, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row(
            "reasoning-only",
            SessionTreeRowKind::Reasoning,
            "hidden chain",
            None,
            Some("reasoning-only"),
        )],
        current_row_id: Some("reasoning-only".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert_eq!(effect, None);
    assert_eq!(
        model.active_toast_text_for_test(),
        Some("No user or assistant messages to copy")
    );
}

#[test]
fn copy_picker_copies_full_text_when_list_render_is_truncated() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(28, 8);
    model.set_palette(default_palette(), true);
    model.open_copy_picker_loading();
    model.apply_copy_picker_payload(SessionTreePayload {
        rows: vec![tree_row(
            "assistant-long",
            SessionTreeRowKind::Assistant,
            "assistant raw text that is much wider than the list viewport",
            None,
            Some("assistant-long"),
        )],
        current_row_id: Some("assistant-long".to_string()),
    });

    let rendered = rendered_rows(&render_model_buffer(&mut model, 28, 8)).join("\n");
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('C'))));

    assert!(
        !rendered.contains("assistant raw text that is much wider than the list viewport"),
        "narrow list should not contain the full row text: {rendered:?}"
    );
    assert_eq!(
        effect,
        Some(AppEffect::CopySelection(
            "assistant raw text that is much wider than the list viewport".to_string()
        ))
    );
}
