use super::*;

#[test]
fn entry_tree_shift_a_requests_branch_tree_from_main_tree_only() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            Some("root question".to_string()),
            Some("user-a"),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('A'))));

    assert_eq!(effect, Some(AppEffect::OpenBranchTree));
}

#[test]
fn entry_tree_shift_a_with_shift_modifier_requests_branch_tree() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            Some("root question".to_string()),
            Some("user-a"),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('A'),
        KeyModifiers::SHIFT,
    )));

    assert_eq!(effect, Some(AppEffect::OpenBranchTree));
}

#[test]
fn entry_tree_shift_a_does_not_open_branch_tree_inside_message_preview() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "user-a",
            SessionTreeRowKind::User,
            "root body",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::User,
                content: "root body".to_string(),
            }],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('A'))));

    assert_eq!(effect, None);
    assert!(model.entry_tree_preview_active());
}

#[test]
fn entry_tree_branch_tree_renders_complete_connectors_and_summary() {
    let mut model = ready_model();
    model.set_window(112, 14);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![numbered_tree_row(0)],
        current_row_id: Some("row-0".to_string()),
    });
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());

    let rows = rendered_rows(&render_model_buffer(&mut model, 112, 14));

    assert!(
        rows[0].starts_with("  Branch Tree (3 of 5)") && !rows[0].contains("Page"),
        "branch tree should select the current branch node by default: {rows:?}"
    );
    assert_eq!(
        rows[usize::from(ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT)].trim(),
        ".",
        "branch tree should render a non-selectable dot root: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("├── 6 msgs root branch")),
        "top-level non-last branch should use tree-style tee connector and message count: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("│   ├── 2 msgs child one")),
        "nested non-last branch should keep the ancestor vertical lane and message count: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("│   └── 3 msgs (current) child two")),
        "nested last branch should render an elbow while preserving the parent lane and current marker: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("└── 3 msgs second root")),
        "last top-level branch should use the final elbow connector and message count: {rows:?}"
    );
    assert!(
        rows.iter()
            .filter(|row| {
                row.contains("root branch")
                    || row.contains("child one")
                    || row.contains("child two")
                    || row.contains("grand child")
                    || row.contains("second root")
            })
            .all(|row| !row.contains('·')),
        "branch tree rows should not show created/updated relative times: {rows:?}"
    );
    assert!(
        rows[8].trim().is_empty()
            && rows[9].contains("5 branches, 9 messages")
            && rows[12].contains(" Page 1/1 "),
        "summary should follow the rendered tree with one blank gap, and page rule should stay above the footer: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("5 branches, 9 messages")),
        "branch tree should show unique logical message summary after the tree output: {rows:?}"
    );
    assert!(
        rows.last()
            .is_some_and(|row| !row.contains("Space preview branch")),
        "current branch footer should not advertise Space preview: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_tree_space_noops_for_current_branch() {
    let mut model = ready_model();
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_eq!(effect, None);
    assert!(!model.entry_tree_branch_preview_active());
}

#[test]
fn entry_tree_branch_tree_space_previews_non_current_branch() {
    let mut model = ready_model();
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_open_branch_preview_effect(
        &model,
        effect,
        "child-one",
        "Space should request preview for selected non-current branch",
    );
    assert!(model.entry_tree_branch_preview_active());
}

#[test]
fn entry_tree_branch_tree_preview_title_shows_branch_metadata() {
    let mut model = ready_model();
    model.set_window(112, 12);
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_open_branch_preview_effect(
        &model,
        effect,
        "child-one",
        "Space should request preview for selected non-current branch",
    );

    model.apply_entry_tree_branch_preview_payload(SessionTreePayload {
        rows: vec![tree_row_with_parent_at_depth(
            "assistant-b",
            None,
            SessionTreeRowKind::Assistant,
            "inactive branch answer",
            0,
            true,
            true,
        )],
        current_row_id: Some("assistant-b".to_string()),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 112, 12));

    assert!(
        rows[0].contains("Branch Preview")
            && !rows[0].contains("msgs")
            && rows[0].contains("Created")
            && rows[0].contains("Updated"),
        "branch tree preview title should show time metadata without repeating message count: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_tree_enter_switches_non_current_branch() {
    let mut model = ready_model();
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Up)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SwitchBranch {
            leaf_id: "leaf-child-one".to_string()
        })
    );
}

#[test]
fn late_branch_tree_payload_after_exit_does_not_reopen_branch_tree() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![numbered_tree_row(0)],
        current_row_id: Some("row-0".to_string()),
    });
    model.open_entry_tree_branch_tree_loading();

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.entry_tree_branch_tree_active());

    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());

    assert!(model.entry_tree_active());
    assert!(!model.entry_tree_branch_tree_active());
}

#[test]
fn branch_tree_loading_state_tracks_only_pending_branch_tree_payload() {
    let mut model = ready_model();

    assert!(!model.entry_tree_branch_tree_loading());

    model.open_entry_tree_branch_tree_loading();
    assert!(model.entry_tree_branch_tree_loading());

    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());
    assert!(!model.entry_tree_branch_tree_loading());
    assert!(model.entry_tree_branch_tree_active());
}

#[test]
fn branch_tree_load_failure_renders_overlay_error() {
    let mut model = ready_model();
    model.open_entry_tree_branch_tree_loading();
    let request_id = model
        .entry_tree_branch_tree_pending_request_id_for_test()
        .unwrap();

    model.apply_runtime_event(RuntimeEvent::SessionBranchTreeLoadFailed {
        request_id,
        message: "branch tree index is corrupt".to_string(),
    });

    assert!(!model.entry_tree_branch_tree_loading());
    assert!(model.entry_tree_branch_tree_active());
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 10));
    assert!(
        rows.iter()
            .any(|row| row.contains("branch tree index is corrupt")),
        "branch tree load failure should render inside the active overlay: {rows:?}"
    );
}

#[test]
fn stale_branch_tree_payload_is_ignored_after_branch_tree_reopens_loading() {
    let mut model = ready_model();

    let stale_request_id = model.open_entry_tree_branch_tree_loading();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    let current_request_id = model.open_entry_tree_branch_tree_loading();

    let mut stale_payload = branch_tree_payload();
    stale_payload.nodes[0].branch.branch_row_id = "stale-root".to_string();
    model.apply_runtime_event(RuntimeEvent::SessionBranchTreeLoaded {
        request_id: stale_request_id,
        payload: stale_payload,
    });

    assert!(model.entry_tree_branch_tree_loading());
    assert_eq!(
        model.entry_tree_branch_tree_row_ids_for_test(),
        Vec::<&str>::new()
    );

    let mut current_payload = branch_tree_payload();
    current_payload.nodes[0].branch.branch_row_id = "current-root".to_string();
    model.apply_runtime_event(RuntimeEvent::SessionBranchTreeLoaded {
        request_id: current_request_id,
        payload: current_payload,
    });

    assert!(!model.entry_tree_branch_tree_loading());
    assert!(
        model
            .entry_tree_branch_tree_row_ids_for_test()
            .contains(&"current-root")
    );
}

#[test]
fn entry_tree_branch_tree_left_click_selects_visible_branch_node() {
    let mut model = ready_model();
    model.set_window(112, 14);
    model.open_entry_tree_branch_tree_loading();
    model.apply_entry_tree_branch_tree_payload(branch_tree_payload());

    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let child_one_row = body_top + 2;
    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 10,
        row: child_one_row,
    });
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_open_branch_preview_effect(
        &model,
        effect,
        "child-one",
        "clicking a visible branch tree row should move branch selection before Space",
    );
}

#[test]
fn entry_tree_footer_mentions_shift_a_branch_tree() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            Some("root question".to_string()),
            Some("user-a"),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    let rows = rendered_rows(&render_model_buffer(&mut model, 96, 12));

    assert!(
        rows.last().is_some_and(|row| row.contains("A branch tree")),
        "tree footer should advertise Shift+A/A branch tree access: {rows:?}"
    );
}
