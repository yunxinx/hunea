use super::*;

#[test]
fn entry_tree_tab_opens_branch_picker_for_selected_fork_row() {
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

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    assert_eq!(effect, None);
    assert!(
        rows.iter().any(|row| row.contains("Switch branch")),
        "Tab on fork row should open branch picker: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("inactive answer"))
            && rows
                .iter()
                .any(|row| row.contains("current follow up") && row.contains("(current)")),
        "branch picker should render latest summaries and current marker: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "branch picker should not render a separate left arrow marker when row highlight already shows focus: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Enter switch · Space preview branch · Esc back")),
        "branch picker should render English footer hints: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_renders_metadata_header_and_rows() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    let now_ms = current_unix_timestamp_ms();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice_with_metadata(
                    "assistant-b",
                    "assistant-b",
                    "inactive answer",
                    false,
                    2,
                    now_ms - 180_000,
                    now_ms - 60_000,
                ),
                branch_choice_with_metadata(
                    "assistant-c",
                    "user-d",
                    "current follow up",
                    true,
                    12,
                    now_ms - 3_600_000,
                    now_ms - 1_800_000,
                ),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 96, 12);
    let rows = rendered_rows(&buffer);
    let header_row = rows
        .iter()
        .position(|row| row.contains("Msgs") && row.contains("Updated"))
        .expect("branch picker table header should render");
    let header_column = column_of_text(&buffer, header_row as u16, "Msgs");
    let inactive_row = rows
        .iter()
        .find(|row| row.contains("inactive answer"))
        .expect("inactive branch row should render");

    assert!(
        rows[header_row].contains("Msgs")
            && rows[header_row].contains("Created")
            && rows[header_row].contains("Updated"),
        "branch picker should render the metadata table header without a redundant Branch label: {rows:?}"
    );
    assert!(
        !rows[header_row].contains("Branch"),
        "branch picker header should not include a redundant Branch label: {rows:?}"
    );
    assert_eq!(
        buffer[(header_column, header_row as u16)].fg,
        table_header_text_style(palette)
            .fg
            .expect("default palette should provide table header color"),
        "branch picker header should use the shared table header style"
    );
    assert!(
        inactive_row.contains("2")
            && inactive_row.contains("3m·00s")
            && inactive_row.contains("1m·00s")
            && inactive_row.contains("inactive answer"),
        "branch picker row should render message count, created age, updated age, and branch content: {inactive_row:?}"
    );
    assert!(
        !inactive_row.contains("assistant") && !inactive_row.contains("user"),
        "branch picker row should not render the message-kind attribute column: {inactive_row:?}"
    );
}

#[test]
fn entry_tree_branch_picker_keeps_two_cell_right_padding() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice(
                    "assistant-b",
                    "assistant-b",
                    "inactive branch answer with enough extra text to reach the picker edge and require truncation before padding",
                    false,
                ),
                branch_choice("assistant-c", "user-d", "current follow up", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let buffer = render_model_buffer(&mut model, 64, 12);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let picker_top = body_top + 1;

    for row in picker_top + 1..=picker_top + 4 {
        assert_eq!(
            buffer[(62, row)].symbol(),
            " ",
            "branch picker should keep a right-side 2-cell padding at row {row}"
        );
        assert_eq!(
            buffer[(63, row)].symbol(),
            " ",
            "branch picker should keep a right-side 2-cell padding at row {row}"
        );
    }
}

#[test]
fn entry_tree_branch_picker_title_and_footer_rules_ignore_right_padding() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
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
    let buffer = render_model_buffer(&mut model, 64, 12);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let picker_top = body_top + 1;
    let title_row = picker_top;
    let footer_row = picker_top + 5;

    for row in [title_row, footer_row] {
        for column in 62..=63 {
            assert_eq!(
                buffer[(column, row)].symbol(),
                "─",
                "branch picker title/footer rule should extend through right padding at row {row}, column {column}"
            );
        }
    }
}

#[test]
fn entry_tree_branch_picker_selected_highlight_extends_into_right_padding() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
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
    let buffer = render_model_buffer(&mut model, 64, 12);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let picker_top = body_top + 1;
    let selected_item_row = picker_top + BRANCH_PICKER_ITEM_TOP_OFFSET;

    for column in 62..=63 {
        assert_eq!(
            buffer[(column, selected_item_row)].symbol(),
            " ",
            "branch picker right padding should stay blank at column {column}"
        );
        assert!(
            buffer[(column, selected_item_row)]
                .modifier
                .contains(Modifier::REVERSED),
            "selected branch picker row highlight should extend into right padding at column {column}"
        );
    }
}

#[test]
fn entry_tree_branch_picker_uses_removed_kind_column_for_current_marker_only() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                branch_choice(
                    "assistant-c",
                    "user-d",
                    "current follow up with enough extra text to push the marker past the row width",
                    true,
                ),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let buffer = render_model_buffer(&mut model, 80, 12);
    let rows = rendered_rows(&buffer);
    let inactive_row = rows
        .iter()
        .position(|row| row.contains("inactive answer"))
        .expect("inactive branch row should render");
    let current_row = rows
        .iter()
        .position(|row| row.contains("(current)"))
        .expect("current marker should remain visible for a long current branch row");
    let inactive_summary_column = column_of_text(&buffer, inactive_row as u16, "inactive answer");
    let current_marker_column = column_of_text(&buffer, current_row as u16, "(current)");
    let current_summary_column = column_of_text(&buffer, current_row as u16, "current follow up");

    assert_eq!(
        inactive_summary_column, current_marker_column,
        "non-current rows should not reserve the current-marker column: {rows:?}"
    );
    assert_eq!(
        current_summary_column,
        current_marker_column + "(current) ".len() as u16,
        "current marker should occupy the removed kind column before the branch summary: {rows:?}"
    );
    assert!(
        rows[inactive_row].contains("inactive answer")
            && !rows[inactive_row].contains("(current)")
            && !rows[inactive_row].contains("assistant"),
        "non-current row should render content directly after metadata without kind/current placeholders: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_uses_open_time_snapshot_for_relative_times() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    let snapshot_now_ms = 1_000_000_000_000;
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice_with_metadata(
                    "assistant-b",
                    "assistant-b",
                    "inactive answer",
                    false,
                    2,
                    snapshot_now_ms - 120_000,
                    snapshot_now_ms - 60_000,
                ),
                branch_choice_with_metadata(
                    "assistant-c",
                    "user-d",
                    "current follow up",
                    true,
                    1,
                    snapshot_now_ms - 300_000,
                    snapshot_now_ms - 240_000,
                ),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    model
        .entry_tree
        .as_mut()
        .and_then(|state| state.branch_picker.as_mut())
        .expect("branch picker should be open")
        .metadata_now_ms = snapshot_now_ms;
    let rows = rendered_rows(&render_model_buffer(&mut model, 96, 12));
    let inactive_row = rows
        .iter()
        .find(|row| row.contains("inactive answer"))
        .expect("inactive branch row should render");

    assert!(
        inactive_row.contains("2m·00s") && inactive_row.contains("1m·00s"),
        "branch picker should render relative times from the picker snapshot time, not the current render time: {inactive_row:?}"
    );
}

#[test]
fn entry_tree_branch_picker_relative_age_uses_shared_label_strategy() {
    use crate::relative_age::relative_age_label;

    let now_ms = 1_800_000_000_000;
    let cases = [
        (now_ms, "now"),
        (now_ms - 42_000, "42s"),
        (now_ms - 5_000, "05s"),
        (now_ms - 125_000, "2m·05s"),
        (now_ms - 7_200_000, "2h·00m"),
        (now_ms - (3 * 86_400_000 + 125_000), "3d·02m"),
        (now_ms - 90 * 86_400_000, "3M·00d"),
        (now_ms - 300 * 86_400_000, "10M·00d"),
        (now_ms - 400 * 86_400_000, "1y·01M"),
    ];
    for (ts, expected) in cases {
        assert_eq!(relative_age_label(now_ms, ts), expected);
        assert_eq!(
            crate::entry_tree::render::branch_picker_relative_age_label(now_ms, ts),
            expected
        );
    }
}

#[test]
fn entry_tree_footer_mentions_tab_when_selected_row_can_open_branch_picker() {
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

    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    assert!(
        rows.last().is_some_and(|row| row.contains("Tab branch")),
        "tree footer should advertise Tab only when the selected row can open the branch picker: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_opens_below_selected_fork_row_with_rule_chrome() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            numbered_tree_row(0),
            numbered_tree_row(1),
            tree_row_with_branch_choices(
                "user-a",
                SessionTreeRowKind::User,
                "root question",
                vec![
                    branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                    branch_choice("assistant-c", "user-d", "current follow up", true),
                ],
            ),
        ],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let palette = *model.palette();
    let buffer = render_model_buffer(&mut model, 72, 16);
    let rows = rendered_rows(&buffer);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let fork_row = body_top + 2;
    let picker_top_row = usize::from(fork_row + 1);
    let footer_row = picker_top_row + 5;
    let footer_hint_column = column_of_text(&buffer, footer_row as u16, "Enter switch");

    assert!(
        rows[picker_top_row].contains("─ Switch branch"),
        "picker rule should open directly below the selected fork row: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Branch point")),
        "picker should not render a separate branch point context row: {rows:?}"
    );
    assert!(
        rows[footer_row].contains("Enter switch · Space preview branch · Esc back"),
        "picker hints should remain inside the rule chrome: {rows:?}"
    );
    assert!(
        rows[footer_row].contains("─ Enter switch")
            && !rows[footer_row].contains("─  Enter switch"),
        "picker footer rule should use exactly one cell between the rule and hint: {rows:?}"
    );
    assert_eq!(
        buffer[(0, picker_top_row as u16)].fg,
        palette.accent,
        "picker title rule should use the palette accent color"
    );
    assert_eq!(
        buffer[(0, footer_row as u16)].fg,
        palette.accent,
        "picker footer rule should color only the rule with the palette accent"
    );
    assert_eq!(
        buffer[(footer_hint_column, footer_row as u16)].fg,
        tertiary_text_style(palette)
            .fg
            .expect("default palette should provide tertiary color"),
        "footer hint text should keep its existing style inside the rule"
    );
    assert!(
        buffer[(footer_hint_column, footer_row as u16)]
            .modifier
            .contains(Modifier::ITALIC),
        "footer hint text should remain italic"
    );
}

#[test]
fn entry_tree_branch_picker_clears_underlying_tree_content() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_branch_choices(
                "user-a",
                SessionTreeRowKind::User,
                "root question",
                vec![
                    branch_choice("assistant-b", "assistant-b", "inactive answer", false),
                    branch_choice("assistant-c", "user-d", "current follow up", true),
                ],
            ),
            tree_row(
                "under-a",
                SessionTreeRowKind::User,
                "underlying content before popup should be hidden",
                None,
                Some("under-a"),
            ),
            tree_row(
                "under-b",
                SessionTreeRowKind::Assistant,
                "underlying branch picker content must not bleed through",
                None,
                Some("under-b"),
            ),
        ],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let buffer = render_model_buffer(&mut model, 72, 12);
    let rows = rendered_rows(&buffer);
    let context_row = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT + 2;

    assert!(
        !rows[usize::from(context_row)].contains("bleed through"),
        "underlying tree content should be cleared from the picker area: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_opens_above_selected_fork_row_when_below_would_overflow() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    let mut rows = (0..9).map(numbered_tree_row).collect::<Vec<_>>();
    rows.push(tree_row_with_branch_choices(
        "user-a",
        SessionTreeRowKind::User,
        "bottom fork",
        vec![
            branch_choice("assistant-b", "assistant-b", "inactive answer", false),
            branch_choice("assistant-c", "user-d", "current follow up", true),
        ],
    ));
    model.apply_entry_tree_payload(SessionTreePayload {
        rows,
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let rendered_rows = rendered_rows(&render_model_buffer(&mut model, 72, 14));
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let fork_row = body_top + 9;
    let picker_height = 6;
    let picker_top_row = usize::from(fork_row - picker_height);

    assert!(
        rendered_rows[picker_top_row].contains("─ Switch branch"),
        "picker should fall back above the selected fork row when below has no room: {rendered_rows:?}"
    );
    assert!(
        rendered_rows[picker_top_row + picker_height as usize - 1].contains("Enter switch"),
        "fallback popup should keep the footer hint inside the bottom rule: {rendered_rows:?}"
    );
}

#[test]
fn entry_tree_tab_noops_when_selected_row_has_no_switchable_siblings() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![branch_choice(
                "assistant-c",
                "user-d",
                "current follow up",
                true,
            )],
        )],
        current_row_id: Some("user-a".to_string()),
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    assert_eq!(effect, None);
    assert!(
        rows.iter().all(|row| !row.contains("Switch branch")),
        "Tab should silently no-op when fewer than two branch choices exist: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_space_requests_preview_for_focused_branch() {
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

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_open_branch_preview_effect(
        &model,
        effect,
        "assistant-b",
        "Space should request preview for focused branch",
    );
}

#[test]
fn entry_tree_branch_picker_space_noops_for_current_branch() {
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
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    assert_eq!(effect, None);
    assert!(
        !model.entry_tree_branch_preview_active(),
        "Space should not open a preview for the already-current branch"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Enter switch · Esc back")),
        "current branch footer should still show switch/back hints: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Space preview branch")),
        "current branch footer should not advertise Space preview: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_enter_requests_switch_branch_for_focused_branch() {
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

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(
        effect,
        Some(AppEffect::SwitchBranch {
            leaf_id: "assistant-b".to_string()
        }),
        "L2 Enter must be a distinct switch-branch effect, not entry rewind"
    );
}

#[test]
fn entry_tree_branch_picker_respects_configured_visible_rows() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            (0..5)
                .map(|index| {
                    branch_choice(
                        &format!("assistant-{index}"),
                        &format!("assistant-{index}"),
                        &format!("branch answer {index}"),
                        index == 4,
                    )
                })
                .collect(),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    assert!(
        rows.iter().any(|row| row.contains("branch answer 0"))
            && rows.iter().any(|row| row.contains("branch answer 2"))
            && rows.iter().all(|row| !row.contains("branch answer 3")),
        "picker should render only configured visible branch rows before scrolling: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_renders_file_picker_style_scrollbar_when_overflowing() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            (0..8)
                .map(|index| {
                    branch_choice(
                        &format!("assistant-{index}"),
                        &format!("assistant-{index}"),
                        &format!("branch answer {index}"),
                        index == 7,
                    )
                })
                .collect(),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let buffer = render_model_buffer(&mut model, 72, 12);
    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let picker_top = body_top + 1;
    let picker_bottom = picker_top + 6;
    let scrollbar_column = 71;
    let scrollbar_symbols = (picker_top..picker_bottom)
        .map(|row| buffer[(scrollbar_column, row)].symbol().to_string())
        .collect::<Vec<_>>();

    assert!(
        scrollbar_symbols.iter().any(|symbol| symbol == "█"),
        "overflowing picker should render the same thumb symbol as the file picker: {scrollbar_symbols:?}"
    );
    assert!(
        scrollbar_symbols
            .iter()
            .all(|symbol| symbol != "↑" && symbol != "↓"),
        "picker scrollbar should not render arrow endpoint symbols: {scrollbar_symbols:?}"
    );
}

#[test]
fn entry_tree_branch_picker_popup_height_uses_three_chrome_rows_plus_configured_list() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            (0..5)
                .map(|index| {
                    branch_choice(
                        &format!("assistant-{index}"),
                        &format!("assistant-{index}"),
                        &format!("branch answer {index}"),
                        index == 4,
                    )
                })
                .collect(),
        )],
        current_row_id: Some("user-a".to_string()),
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    let rows = rendered_rows(&render_model_buffer(&mut model, 72, 12));

    let title_row = rows
        .iter()
        .position(|row| row.contains("─ Switch branch"))
        .expect("picker title should render");
    let footer_row = rows
        .iter()
        .position(|row| row.contains("Enter switch"))
        .expect("picker footer should render");
    assert_eq!(
        footer_row - title_row + 1,
        6,
        "N=3 list rows should render inside 3+N total popup rows: {rows:?}"
    );
    assert!(
        rows[footer_row].contains('─'),
        "picker hint footer should be rendered inside the bottom rule chrome: {rows:?}"
    );
}

#[test]
fn entry_tree_branch_picker_mouse_wheel_moves_focused_branch() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_branch_choices(
            "user-a",
            SessionTreeRowKind::User,
            "root question",
            vec![
                branch_choice("assistant-a", "leaf-a", "branch answer 0", false),
                branch_choice("assistant-b", "leaf-b", "branch answer 1", false),
                branch_choice("assistant-c", "leaf-c", "branch answer 2", true),
            ],
        )],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_open_branch_preview_effect(
        &model,
        effect,
        "assistant-b",
        "wheel down in L2 should move focus before Space opens preview",
    );
}

#[test]
fn entry_tree_branch_picker_left_click_selects_branch_item() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.set_window(72, 12);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![
            tree_row_with_branch_choices(
                "user-a",
                SessionTreeRowKind::User,
                "root question",
                vec![
                    branch_choice("assistant-a", "leaf-a", "branch answer 0", false),
                    branch_choice("assistant-b", "leaf-b", "branch answer 1", false),
                    branch_choice("assistant-c", "leaf-c", "branch answer 2", true),
                ],
            ),
            numbered_tree_row(1),
            numbered_tree_row(2),
        ],
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let picker_top = body_top + 1;
    let second_item_row = picker_top + 3;
    let click_effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 8,
        row: second_item_row,
    });
    let preview_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert_eq!(click_effect, None);
    assert_open_branch_preview_effect(
        &model,
        preview_effect,
        "assistant-b",
        "left click inside the picker should select that branch item instead of selecting the tree row below",
    );
}

#[test]
fn entry_tree_branch_picker_left_click_visible_tree_row_selects_row_and_closes_picker() {
    let mut model = ready_model_with_options(ModelOptions {
        branch_picker_list_rows: 3,
        ..ModelOptions::default()
    });
    model.set_window(72, 14);
    model.open_entry_tree_loading();
    let mut rows = vec![tree_row_with_branch_choices(
        "user-a",
        SessionTreeRowKind::User,
        "root question",
        vec![
            branch_choice("assistant-a", "leaf-a", "branch answer 0", false),
            branch_choice("assistant-b", "leaf-b", "branch answer 1", false),
            branch_choice("assistant-c", "leaf-c", "branch answer 2", true),
        ],
    )];
    rows.extend((1..10).map(numbered_tree_row));
    model.apply_entry_tree_payload(SessionTreePayload {
        rows,
        current_row_id: Some("user-a".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    let body_top = ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT;
    let visible_tree_row_below_picker = body_top + 7;
    let effect = model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: visible_tree_row_below_picker,
    });
    let rendered_rows = rendered_rows(&render_model_buffer(&mut model, 72, 14));

    assert_eq!(effect, None);
    assert!(
        !model.entry_tree_branch_picker_active(),
        "clicking a visible tree row outside the picker should close the picker"
    );
    assert!(
        rendered_rows[0].starts_with("  Session Tree (8 of 10)"),
        "clicking a visible tree row outside the picker should still select that tree row: {rendered_rows:?}"
    );
    assert!(
        rendered_rows
            .iter()
            .all(|row| !row.contains("Switch branch")),
        "closed picker should no longer render branch picker chrome: {rendered_rows:?}"
    );
}
