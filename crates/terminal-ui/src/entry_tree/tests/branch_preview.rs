use super::*;

#[test]
fn entry_tree_branch_preview_renders_payload_and_returns_to_picker() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_parent_at_depth(
                "user-a",
                None,
                SessionTreeRowKind::User,
                "root question",
                0,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive answer",
                1,
                true,
                true,
            ),
        ],
        current_row_id: Some("assistant-b".to_string()),
    });
    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 72, 10);
    let rows = rendered_rows(&buffer);
    let title_column = column_of_text(&buffer, 0, "Branch Preview");
    assert!(
        rows[0].contains("Branch Preview")
            && rows.iter().any(|row| row.contains("inactive answer")),
        "branch preview should render the preview path payload: {rows:?}"
    );
    assert!(
        !rows[0].contains("msgs") && !rows[0].contains("Created") && !rows[0].contains("Updated"),
        "branch picker preview title should not repeat metadata already shown in the picker: {rows:?}"
    );
    assert_eq!(
        buffer[(title_column, 0)].fg,
        approval_rejected_text_style(palette)
            .fg
            .expect("default palette should provide approval rejected color"),
        "branch preview title should use the existing yellow approval-rejected color"
    );

    let enter_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(enter_effect, None, "Enter in L3 must be a no-op");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    let picker_rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));
    assert!(
        picker_rows.iter().any(|row| row.contains("Switch branch")),
        "Esc in L3 should return to the L2 branch picker: {picker_rows:?}"
    );
}

#[test]
fn entry_tree_branch_preview_left_click_selects_visible_row() {
    let mut model = ready_model();
    model.set_window(72, 10);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![
            SessionTreeRow {
                preview_replay_items: vec![TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::User,
                    content: "root question body".to_string(),
                }],
                ..tree_row_with_parent_at_depth(
                    "user-a",
                    None,
                    SessionTreeRowKind::User,
                    "root question body",
                    0,
                    true,
                    false,
                )
            },
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive answer body",
                1,
                true,
                false,
            ),
            tree_row_with_parent_at_depth(
                "tool-c",
                Some("assistant-b"),
                SessionTreeRowKind::Tool,
                "tool result body",
                2,
                true,
                true,
            ),
        ],
        current_row_id: Some("user-a".to_string()),
    });

    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT + 2,
    });
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 10));

    assert_eq!(effect, None);
    assert!(
        rows[0].starts_with("  Branch Preview (3 of 3"),
        "clicking a visible branch preview row should move the branch preview selection: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_preview_mouse_wheel_moves_preview_selection() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![
            SessionTreeRow {
                preview_replay_items: vec![TranscriptReplayItem::Message {
                    role: TranscriptReplayRole::User,
                    content: "root question body".to_string(),
                }],
                ..tree_row_with_parent_at_depth(
                    "user-a",
                    None,
                    SessionTreeRowKind::User,
                    "root question body",
                    0,
                    true,
                    false,
                )
            },
            tree_row_with_parent_at_depth(
                "assistant-b",
                Some("user-a"),
                SessionTreeRowKind::Assistant,
                "inactive answer body",
                1,
                true,
                true,
            ),
        ],
        current_row_id: Some("assistant-b".to_string()),
    });

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 10));

    assert_eq!(effect, None);
    assert!(model.entry_tree_preview_active());
    assert!(
        rows.iter().any(|row| row.contains("root question body")),
        "wheel up in L3 should move selection before Space opens L4: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_preview_space_opens_message_preview_and_esc_returns_to_l3() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![tree_row_with_parent_at_depth(
            "assistant-b",
            None,
            SessionTreeRowKind::Assistant,
            "inactive answer",
            0,
            true,
            true,
        )],
        current_row_id: Some("assistant-b".to_string()),
    });

    let space_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(space_effect, None);
    assert!(model.entry_tree_preview_active());

    let enter_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(enter_effect, None, "Enter in L4 must be ignored");
    assert!(model.entry_tree_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_preview_active());
    assert!(model.entry_tree_branch_preview_active());
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 10));
    assert!(
        rows[0].contains("Branch Preview"),
        "Esc from L4 should return to L3 branch preview: {rows:?}"
    );
}

#[test]
fn late_branch_preview_payload_after_exit_does_not_reopen_branch_preview() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_branch_preview_active());
    assert!(model.entry_tree_branch_picker_active());

    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![tree_row_with_parent_at_depth(
            "assistant-b",
            None,
            SessionTreeRowKind::Assistant,
            "inactive answer",
            0,
            true,
            true,
        )],
        current_row_id: Some("assistant-b".to_string()),
    });

    assert!(model.entry_tree_branch_picker_active());
    assert!(!model.entry_tree_branch_preview_active());
}

#[test]
fn branch_preview_loading_state_tracks_only_pending_branch_preview_payload() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    assert!(!model.entry_tree_branch_preview_loading());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.entry_tree_branch_preview_loading());

    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![tree_row_with_parent_at_depth(
            "assistant-b",
            None,
            SessionTreeRowKind::Assistant,
            "inactive answer",
            0,
            true,
            true,
        )],
        current_row_id: Some("assistant-b".to_string()),
    });

    assert!(!model.entry_tree_branch_preview_loading());
    assert!(model.entry_tree_branch_preview_active());
}

#[test]
fn entry_tree_esc_pops_l4_l3_l2_l1_in_order() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![tree_row_with_parent_at_depth(
            "assistant-b",
            None,
            SessionTreeRowKind::Assistant,
            "inactive answer",
            0,
            true,
            true,
        )],
        current_row_id: Some("assistant-b".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.entry_tree_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_preview_active());
    assert!(model.entry_tree_branch_preview_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_branch_preview_active());
    assert!(model.entry_tree_branch_picker_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_branch_picker_active());
    assert!(model.entry_tree_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_active());
}
