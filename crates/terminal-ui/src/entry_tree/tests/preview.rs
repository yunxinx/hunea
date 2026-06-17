use super::*;

#[test]
fn entry_tree_space_opens_single_message_preview_and_enter_is_ignored() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            &(0..12)
                .map(|index| format!("preview line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: (0..12)
                    .map(|index| format!("preview line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }],
        )],
        current_row_id: None,
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(effect, None);
    assert!(model.entry_tree_preview_active());

    let preview_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        preview_rows
            .iter()
            .any(|row| row.contains("preview line 11")),
        "preview should show the selected row's full message and start on latest page: {preview_rows:?}"
    );
    assert!(
        preview_rows.iter().any(|row| row.contains(" Page ")),
        "preview should use the same page rule style as resume preview: {preview_rows:?}"
    );

    let enter_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(
        enter_effect, None,
        "Enter inside preview must not execute rewind"
    );
    assert!(model.entry_tree_preview_active());
    assert!(model.entry_tree_active());

    let close_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert_eq!(close_effect, None);
    assert!(!model.entry_tree_preview_active());
    assert!(model.entry_tree_active());
}

#[test]
fn entry_tree_space_preview_does_not_fallback_to_legacy_preview_content() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            "legacy preview content",
            None,
            Some("assistant-a"),
        )],
        current_row_id: None,
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 80, 10));

    assert!(
        !rows
            .iter()
            .any(|row| row.contains("legacy preview content")),
        "tree preview should only render explicit replay items: {rows:?}"
    );
}

#[test]
fn entry_tree_space_preview_renders_assistant_tool_call_json_with_highlighting() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            "I will inspect the file.",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: concat!(
                    "I will inspect the file.\n\n",
                    "Tool call `read_file` (call-1)\n",
                    "```json\n",
                    "{\n",
                    "  \"path\": \"Cargo.toml\"\n",
                    "}\n",
                    "```"
                )
                .to_string(),
            }],
        )],
        current_row_id: None,
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let buffer = render_model_buffer(&mut model, 80, 14);
    let rows = rendered_rows(&buffer);

    assert!(
        rows.iter()
            .any(|row| row.contains("Tool call") && row.contains("read_file")),
        "assistant tree preview should include tool call metadata: {rows:?}"
    );
    let json_row = rows
        .iter()
        .position(|row| row.contains("\"path\"") && row.contains("Cargo.toml"))
        .expect("assistant tool call JSON should be visible in preview");
    assert!(
        (0..buffer.area.width).any(|column| buffer[(column, json_row as u16)].fg != Color::Reset),
        "assistant tool call JSON should retain syntax highlight colors: {rows:?}"
    );
}

#[test]
fn entry_tree_preview_render_does_not_change_scroll_offset() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-long",
            SessionTreeRowKind::Assistant,
            "assistant preview",
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: (0..20)
                    .map(|index| format!("preview line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }],
        )],
        current_row_id: Some("assistant-long".to_string()),
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    assert!(model.entry_tree_preview_active());

    let preview = model
        .entry_tree
        .as_mut()
        .and_then(|state| state.preview.as_mut())
        .expect("entry tree preview should be open");
    preview.is_following_bottom = true;
    preview.overlay.scroll_offset = 0;

    let _ = render_model_buffer(&mut model, 60, 8);

    let scroll_offset = model
        .entry_tree
        .as_ref()
        .and_then(|state| state.preview.as_ref())
        .map(|preview| preview.overlay.scroll_offset);
    assert_eq!(
        scroll_offset,
        Some(0),
        "rendering must not repair or advance preview scroll state"
    );
}

#[test]
fn entry_tree_space_preview_renders_tool_activity_in_detailed_mode() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "tool-a",
            SessionTreeRowKind::Tool,
            "test output line 8",
            vec![TranscriptReplayItem::ToolActivity {
                activity: RuntimeToolActivity {
                    activity_id: "call-1".to_string(),
                    title: "Run cargo test".to_string(),
                    kind: RuntimeToolKind::Execute,
                    status: RuntimeToolActivityStatus::Completed,
                    content: Vec::new(),
                    locations: Vec::new(),
                    raw_input: Some(r#"{"command":"cargo test"}"#.into()),
                    raw_output: Some(
                        (1..=8)
                            .map(|line| format!("test output line {line}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                            .into(),
                    ),
                },
            }],
        )],
        current_row_id: None,
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 80, 14));

    assert!(
        rows.iter().any(|row| row.contains("$ cargo test")),
        "tool tree preview should show the invoked tool command: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("test output line 7")),
        "tool tree preview should use detailed output instead of compact ctrl+t hint: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| !row.contains("ctrl + t to view transcript")),
        "tool tree preview should not show compact transcript hints: {rows:?}"
    );
}

#[test]
fn entry_tree_space_preview_renders_collapsed_tool_activity_debug_details() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "tool-a",
            SessionTreeRowKind::Tool,
            "lib.rs\ntool_result.rs",
            vec![TranscriptReplayItem::ToolActivity {
                activity: RuntimeToolActivity {
                    activity_id: "call-list".to_string(),
                    title: "List Directory src".to_string(),
                    kind: RuntimeToolKind::Search,
                    status: RuntimeToolActivityStatus::Completed,
                    content: Vec::new(),
                    locations: Vec::new(),
                    raw_input: Some(serde_json::json!({ "path": "src" }).into()),
                    raw_output: Some("lib.rs\ntool_result.rs".into()),
                },
            }],
        )],
        current_row_id: None,
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 80, 14));

    assert!(
        rows.iter().any(|row| row.contains("● List src")),
        "tool tree preview should keep the tool identity visible: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Input")),
        "tool tree preview should expose raw tool input: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("\"path\"")),
        "tool tree preview should include tool argument JSON: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("tool_result.rs")),
        "tool tree preview should expose raw tool output: {rows:?}"
    );
}

#[test]
fn entry_tree_preview_maps_wheel_to_page_navigation_if_delivered() {
    let mut model = ready_model();
    model.set_window(60, 8);
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: vec![tree_row_with_preview_replay_items(
            "assistant-a",
            SessionTreeRowKind::Assistant,
            &(0..12)
                .map(|index| format!("preview line {index}"))
                .collect::<Vec<_>>()
                .join("\n"),
            vec![TranscriptReplayItem::Message {
                role: TranscriptReplayRole::Assistant,
                content: (0..12)
                    .map(|index| format!("preview line {index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            }],
        )],
        current_row_id: None,
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    model.update(AppEvent::MouseWheel { delta_lines: -3 });
    let previous_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        previous_page_rows
            .iter()
            .any(|row| row.contains(" Page 1/2 ")),
        "wheel up should page back inside tree preview: {previous_page_rows:?}"
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });
    let next_page_rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        next_page_rows.iter().any(|row| row.contains(" Page 2/2 ")),
        "wheel down should page forward inside tree preview: {next_page_rows:?}"
    );
}

#[test]
fn entry_tree_preview_keeps_terminal_native_mouse_selection() {
    let mut model = ready_model();
    model.open_entry_tree_loading();
    model.apply_entry_tree_payload(SessionTreePayload {
        rows: (0..3).map(numbered_tree_row).collect(),
        current_row_id: None,
    });

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));

    assert!(model.entry_tree_preview_active());
    assert_eq!(
        model.mouse_mode_preference(),
        TerminalMouseModePreference::NativeWithAlternateScroll,
        "tree preview should keep terminal-native selection while preserving alternate-scroll wheel delivery"
    );

    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 12,
        row: ENTRY_TREE_HEADER_HEIGHT + ENTRY_TREE_HEADER_RULE_HEIGHT,
    });
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(' '))));
    let rows = rendered_rows(&render_model_buffer(&mut model, 60, 8));
    assert!(
        rows[0].starts_with("  Session Tree (3 of 3)"),
        "clicking inside preview must not change the underlying tree selection: {rows:?}"
    );
}
