use super::*;
use crate::{
    AppEffect, AppEvent, Sender, StartupBannerOptions,
    overlay_input_result::OverlayInputResult,
    theme::{default_palette, terminal_default_palette},
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

fn handled_effect(result: OverlayInputResult, context: &str) -> Option<AppEffect> {
    assert!(!result.is_ignored(), "{context}");
    result.into_effect()
}

#[test]
fn preview_layout_omits_labels_and_uses_vertical_numbered_choices() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        lines
            .iter()
            .all(|line| !line.contains("Preview") && !line.contains("Preview tool request")),
        "preview marker and synthetic preview title should not be rendered: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .all(|line| !line.contains("Tool   :") && !line.contains("Request:")),
        "tool and request labels should not be rendered: {lines:?}"
    );
    let header = lines
        .iter()
        .position(|line| line.contains("Tool Approval:"))
        .expect("header should render");
    let command = lines
        .iter()
        .position(|line| line.contains("sed -n"))
        .expect("command row should render");
    let first_choice = lines
        .iter()
        .position(|line| line.contains("1. Yes"))
        .expect("first approval choice should render");
    assert!(
        header < command && command < first_choice,
        "command should sit between header and choices: {lines:?}"
    );
    assert_eq!(
        lines.get(header + 1).map(String::as_str),
        Some(""),
        "header should keep a blank row before the command: {lines:?}"
    );
    assert_eq!(
        first_choice.saturating_sub(command + 1),
        1,
        "choices should keep one blank row after the command when details are absent: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains("Reason")),
        "preview should not synthesize a reason row: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains("Actions:")),
        "shell approval should not use the old actions heading: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line == "  ➜ 1. Yes"),
        "selected choice should use the shared marker and numbering style: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("2. Yes, allow similar requests during this session")),
        "preview should expose the session allow option for design checks: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("4. No, reject similar requests during this session")),
        "preview should expose the session reject option for design checks: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("Esc to cancel · Enter to choose")),
        "footer hint should use the concise approval copy: {lines:?}"
    );
}

#[test]
fn command_line_wraps_without_request_label() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::Preview,
        "cargo clippy --workspace --all-targets -- -D warnings".to_string(),
        vec![ToolApprovalDetail {
            label: "Reason".to_string(),
            value: "Inspect wrapping".to_string(),
        }],
    );

    let lines = build_panel_lines(&mut model, 28)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        lines.iter().all(|line| !line.contains("Request:")),
        "wrapped command should not use a request label: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .filter(|line| line.starts_with("  ") && !line.contains(':'))
            .count()
            > 1
            && lines.iter().any(|line| line.contains("cargo clippy"))
            && lines.iter().any(|line| line.contains("warning")),
        "long command should wrap across multiple display rows: {lines:?}"
    );
}

#[test]
fn long_command_keeps_full_document_flow_without_truncating_choices() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.set_window(24, 8);
    model.open_tool_approval_panel(
        ToolApprovalSource::Preview,
        "cargo run --bin hunea -- --very-long-debug-command-that-wraps".to_string(),
        Vec::new(),
    );

    let panel = model.current_inline_tool_approval_panel_render_result();
    let text = panel.plain_lines.join("\n");

    assert!(
        panel.plain_lines.len() > usize::from(model.height),
        "long wrapped command should remain in document flow for viewport scrolling"
    );
    assert!(
        text.contains("1. Yes") && text.contains("Esc to cancel · Enter to choose"),
        "choices and footer should not be truncated away: {text:?}"
    );
}

#[test]
fn runtime_session_allow_option_only_renders_when_available() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-1".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "touch src/main.rs".to_string(),
        vec![ToolApprovalDetail {
            label: "Reason".to_string(),
            value: "Inspect actions".to_string(),
        }],
    );

    let without_session = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(
        without_session
            .iter()
            .all(|line| !line.contains("allow similar requests")),
        "session allow should not render without an upstream option: {without_session:?}"
    );

    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-2".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: Some("reject-always".to_string()),
        },
        "touch src/main.rs".to_string(),
        vec![ToolApprovalDetail {
            label: "Reason".to_string(),
            value: "Inspect actions".to_string(),
        }],
    );

    let with_session = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_ordered_plain_lines(
        &with_session,
        &[
            "1. Yes",
            "2. Yes, allow similar requests during this session",
            "3. No",
            "4. No, reject similar requests during this session",
        ],
    );
}

#[test]
fn runtime_permission_panel_keeps_dynamic_option_names() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget,
    };

    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: RuntimeTarget::provider("local", "qwen3"),
        request: RuntimePermissionRequest::new(
            "permission-dynamic-options",
            Some("Write TEMP.md".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "reject-always-id",
                    "Keep rejecting matching writes",
                    RuntimePermissionOptionKind::RejectAlways,
                ),
                RuntimePermissionOption::new(
                    "allow-always-id",
                    "Remember this workspace write approval",
                    RuntimePermissionOptionKind::AllowAlways,
                ),
                RuntimePermissionOption::new(
                    "reject-once-id",
                    "Reject this write",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
                RuntimePermissionOption::new(
                    "allow-once-id",
                    "Allow this write",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
            ],
        ),
    });

    let text = build_panel_lines(&mut model, 120)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains("Allow this write"),
        "dynamic allow label missing: {text:?}"
    );
    assert!(
        text.contains("Remember this workspace write approval"),
        "dynamic session allow label missing: {text:?}"
    );
    assert!(
        text.contains("Reject this write"),
        "dynamic reject label missing: {text:?}"
    );
    assert!(
        text.contains("Keep rejecting matching writes"),
        "dynamic session reject label missing: {text:?}"
    );
}

#[test]
fn runtime_permission_panel_numeric_key_uses_option_kind_mapping() {
    use crate::runtime::RuntimeEventApply;
    use runtime_domain::session::{
        RuntimeEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget,
    };

    let target = RuntimeTarget::provider("local", "qwen3");
    let mut model = Model::new(StartupBannerOptions::default());
    model.apply_runtime_event(RuntimeEvent::PermissionRequested {
        target: target.clone(),
        request: RuntimePermissionRequest::new(
            "permission-numeric",
            Some("Write TEMP.md".to_string()),
            vec![
                RuntimePermissionOption::new(
                    "reject-once-id",
                    "Reject once",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
                RuntimePermissionOption::new(
                    "allow-always-id",
                    "Allow in runtime",
                    RuntimePermissionOptionKind::AllowAlways,
                ),
                RuntimePermissionOption::new(
                    "allow-once-id",
                    "Allow once",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    "reject-always-id",
                    "Reject in runtime",
                    RuntimePermissionOptionKind::RejectAlways,
                ),
            ],
        ),
    });

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Char('2').into()),
        "numeric approval selection should be consumed by the modal panel",
    );

    assert_eq!(
        effect,
        Some(AppEffect::RespondRuntimePermission {
            target,
            request_id: "permission-numeric".to_string(),
            option_id: Some("allow-always-id".to_string()),
        })
    );
}

#[test]
fn file_preview_command_bar_keeps_dynamic_runtime_options_in_both_layouts() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(200, 80);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-options".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: Some("allow-always-id".to_string()),
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: Some("reject-always-id".to_string()),
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow this file",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "allow-always-id",
            "Remember file approvals",
            RuntimePermissionOptionKind::AllowAlways,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject this file",
            RuntimePermissionOptionKind::RejectOnce,
        ),
        RuntimePermissionOption::new(
            "reject-always-id",
            "Keep rejecting this file",
            RuntimePermissionOptionKind::RejectAlways,
        ),
    ]);

    let inline = build_panel_lines(&mut model, 200)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert_strictly_ordered_plain_lines(
        &inline,
        &[
            "1. Allow this file",
            "2. Remember file approvals",
            "3. Reject this file",
            "4. Keep rejecting this file",
        ],
    );

    model.set_window(200, 10);
    let fullscreen = rendered_model_rows(&mut model, 200, 10);
    assert_strictly_ordered_plain_lines(
        &fullscreen,
        &[
            "1. Allow this file",
            "2. Remember file approvals",
            "3. Reject this file",
            "4. Keep rejecting this file",
        ],
    );
    assert!(model.tool_approval_fullscreen_preview_active());
    let _ = model.handle_tool_approval_panel_key(KeyCode::Down.into());
    assert_eq!(model.tool_approval_panel.selected, 1);
    let moved_buffer = rendered_model_buffer(&mut model, 200, 10);
    let moved_fullscreen = buffer_rows(&moved_buffer);
    assert!(
        moved_fullscreen
            .iter()
            .any(|line| line.contains("➜ 2. Remember file approvals")),
        "fullscreen selection marker should follow Down: {moved_fullscreen:?}"
    );

    let selected_row = moved_fullscreen
        .iter()
        .position(|line| line.contains("2. Remember file approvals"))
        .expect("selected fullscreen choice should render");
    let unselected_row = moved_fullscreen
        .iter()
        .position(|line| line.contains("1. Allow this file"))
        .expect("unselected fullscreen choice should render");
    let hint_row = moved_fullscreen
        .iter()
        .position(|line| line.contains("Enter choose"))
        .expect("fullscreen action hint should render");
    assert!(
        moved_fullscreen[hint_row.saturating_sub(1)]
            .trim()
            .is_empty(),
        "fullscreen choices and action hint should keep a blank row: {moved_fullscreen:?}"
    );

    let selected_cell = (0..moved_buffer.area.width)
        .map(|column| &moved_buffer[(column, selected_row as u16)])
        .find(|cell| cell.symbol() == "2")
        .expect("selected fullscreen choice should expose a styled value cell");
    assert_eq!(selected_cell.fg, model.palette.main);
    assert!(selected_cell.modifier.contains(Modifier::BOLD));

    let unselected_cell = (0..moved_buffer.area.width)
        .map(|column| &moved_buffer[(column, unselected_row as u16)])
        .find(|cell| cell.symbol() == "1")
        .expect("unselected fullscreen choice should expose a styled value cell");
    assert_eq!(unselected_cell.fg, model.palette.secondary);
    assert!(!unselected_cell.modifier.contains(Modifier::BOLD));

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Enter.into()),
        "fullscreen Enter should apply the selected runtime option",
    )
    .expect("fullscreen runtime permission selection should emit an effect");
    assert_eq!(
        effect,
        AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-options".to_string(),
            option_id: Some("allow-always-id".to_string()),
        }
    );
}

#[test]
fn narrow_file_preview_wraps_each_dynamic_option_explicitly() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(28, 80);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-narrow-options".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: Some("allow-always-id".to_string()),
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: Some("reject-always-id".to_string()),
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow this exceptionally long file change",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "allow-always-id",
            "Remember this file approval",
            RuntimePermissionOptionKind::AllowAlways,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject this file",
            RuntimePermissionOptionKind::RejectOnce,
        ),
        RuntimePermissionOption::new(
            "reject-always-id",
            "Keep rejecting this file",
            RuntimePermissionOptionKind::RejectAlways,
        ),
    ]);

    let lines = build_panel_lines(&mut model, 28)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let first_choice = lines
        .iter()
        .position(|line| line.contains("1. Allow this"))
        .expect("first dynamic choice should render");
    let second_choice = lines
        .iter()
        .position(|line| line.contains("2. Remember"))
        .expect("second dynamic choice should render");

    assert!(
        second_choice >= first_choice + 2,
        "the first dynamic choice should occupy multiple narrow-screen rows: {lines:?}"
    );
    assert!(
        lines[first_choice + 1].contains("exceptionally")
            || lines[first_choice + 1].contains("long file change"),
        "the wrapped continuation should remain readable: {lines:?}"
    );
}

#[test]
fn file_preview_choices_keep_shared_selection_visuals_and_footer_spacing() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(120, 80);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-selection-style".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: Some("allow-always-id".to_string()),
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: Some("reject-always-id".to_string()),
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow this file",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "allow-always-id",
            "Remember file approvals",
            RuntimePermissionOptionKind::AllowAlways,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject this file",
            RuntimePermissionOptionKind::RejectOnce,
        ),
        RuntimePermissionOption::new(
            "reject-always-id",
            "Keep rejecting this file",
            RuntimePermissionOptionKind::RejectAlways,
        ),
    ]);

    let lines = build_panel_lines(&mut model, 120);
    let plain_lines = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let selected_index = plain_lines
        .iter()
        .position(|line| line.contains("1. Allow this file"))
        .expect("selected file approval choice should render");
    let unselected_index = plain_lines
        .iter()
        .position(|line| line.contains("2. Remember file approvals"))
        .expect("unselected file approval choice should render");
    let hint_index = plain_lines
        .iter()
        .position(|line| line.contains("Enter choose"))
        .expect("file approval action hint should render");

    assert_eq!(plain_lines[selected_index], "  ➜ 1. Allow this file");
    assert_eq!(
        plain_lines[unselected_index],
        "    2. Remember file approvals"
    );
    assert_eq!(
        plain_lines.get(hint_index.saturating_sub(1)),
        Some(&String::new()),
        "choices and the action hint should keep one blank row: {plain_lines:?}"
    );

    let selected_value = lines[selected_index]
        .spans
        .iter()
        .find(|span| span.content.contains("1. Allow this file"))
        .expect("selected choice value span should render");
    assert_eq!(selected_value.style.fg, Some(model.palette.main));
    assert!(selected_value.style.add_modifier.contains(Modifier::BOLD));

    let unselected_value = lines[unselected_index]
        .spans
        .iter()
        .find(|span| span.content.contains("2. Remember file approvals"))
        .expect("unselected choice value span should render");
    assert_eq!(unselected_value.style.fg, Some(model.palette.secondary));
    assert!(!unselected_value.style.add_modifier.contains(Modifier::BOLD));

    assert!(
        !model
            .handle_tool_approval_panel_key(KeyCode::Down.into())
            .is_ignored()
    );
    let moved_lines = build_panel_lines(&mut model, 120)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    assert!(
        moved_lines
            .iter()
            .any(|line| line == "    1. Allow this file")
    );
    assert!(
        moved_lines
            .iter()
            .any(|line| line == "  ➜ 2. Remember file approvals")
    );

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Enter.into()),
        "Enter should apply the selected file approval choice",
    )
    .expect("runtime permission selection should emit an effect");
    assert_eq!(
        effect,
        AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-selection-style".to_string(),
            option_id: Some("allow-always-id".to_string()),
        }
    );
}

#[test]
fn file_preview_command_bar_only_renders_available_once_choices() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(200, 80);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-once-options".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow this file",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject this file",
            RuntimePermissionOptionKind::RejectOnce,
        ),
    ]);

    let lines = build_panel_lines(&mut model, 200)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_strictly_ordered_plain_lines(&lines, &["1. Allow this file", "2. Reject this file"]);
    assert!(
        lines
            .iter()
            .all(|line| !line.contains("3.") && !line.contains("similar requests")),
        "unavailable session choices must not be synthesized: {lines:?}"
    );
}

#[test]
fn constrained_fullscreen_file_preview_prioritizes_approval_choices() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(200, 5);
    model.set_palette(default_palette(), true);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-constrained".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: Some("allow-always-id".to_string()),
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: Some("reject-always-id".to_string()),
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow once",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "allow-always-id",
            "Allow in session",
            RuntimePermissionOptionKind::AllowAlways,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject once",
            RuntimePermissionOptionKind::RejectOnce,
        ),
        RuntimePermissionOption::new(
            "reject-always-id",
            "Reject in session",
            RuntimePermissionOptionKind::RejectAlways,
        ),
    ]);

    let rows = rendered_model_rows(&mut model, 200, 5);

    assert_strictly_ordered_plain_lines(
        &rows,
        &[
            "1. Allow once",
            "2. Allow in session",
            "3. Reject once",
            "4. Reject in session",
        ],
    );
    assert!(
        rows.iter().all(|line| !line.contains("PgUp/PgDn")),
        "scroll hints should yield to approval choices in a constrained footer: {rows:?}"
    );
}

#[test]
fn constrained_fullscreen_file_preview_never_places_hint_directly_after_choices() {
    use runtime_domain::session::{RuntimePermissionOption, RuntimePermissionOptionKind};

    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(200, 6);
    model.set_palette(default_palette(), true);
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-preview-constrained-spacing".to_string(),
            allow_option_id: Some("allow-once-id".to_string()),
            allow_always_option_id: Some("allow-always-id".to_string()),
            reject_option_id: Some("reject-once-id".to_string()),
            reject_always_option_id: Some("reject-always-id".to_string()),
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            "body".to_string(),
        )),
    );
    model.set_runtime_permission_options(vec![
        RuntimePermissionOption::new(
            "allow-once-id",
            "Allow once",
            RuntimePermissionOptionKind::AllowOnce,
        ),
        RuntimePermissionOption::new(
            "allow-always-id",
            "Allow in session",
            RuntimePermissionOptionKind::AllowAlways,
        ),
        RuntimePermissionOption::new(
            "reject-once-id",
            "Reject once",
            RuntimePermissionOptionKind::RejectOnce,
        ),
        RuntimePermissionOption::new(
            "reject-always-id",
            "Reject in session",
            RuntimePermissionOptionKind::RejectAlways,
        ),
    ]);

    let rows = rendered_model_rows(&mut model, 200, 6);

    assert_strictly_ordered_plain_lines(
        &rows,
        &[
            "1. Allow once",
            "2. Allow in session",
            "3. Reject once",
            "4. Reject in session",
        ],
    );
    assert!(
        rows.iter().all(|line| !line.contains("Enter choose")),
        "the action hint should yield when there is no room for its required blank row: {rows:?}"
    );
}

#[test]
fn choices_render_vertically_for_command_approval() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert_ordered_plain_lines(
        &lines,
        &[
            "  ➜ 1. Yes",
            "    2. Yes, allow similar requests during this session",
            "    3. No",
            "    4. No, reject similar requests during this session",
        ],
    );
    assert!(
        lines.iter().all(|line| {
            let combines_allow_choices = line.contains("1. Yes") && line.contains("2.");
            let combines_deny_choices = line.contains("3. No") && line.contains("4.");
            !(combines_allow_choices || combines_deny_choices)
        }),
        "each approval choice should occupy its own line: {lines:?}"
    );
}

#[test]
fn preview_choice_closes_without_status_notice_and_appends_result() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Enter.into()),
        "tool approval panel should handle Enter",
    );

    assert!(effect.is_none());
    assert!(!model.tool_approval_panel_active());
    assert!(
        model.current_status_notice_text().is_empty(),
        "preview approval should close silently instead of showing a status notice"
    );
    assert!(
        model
            .transcript_mut()
            .plain_items()
            .iter()
            .any(|item| item == "● Ran sed -n '1,80p' src/main.rs"),
        "preview approval should append a testable tool result to transcript"
    );
}

#[test]
fn runtime_allow_choice_does_not_append_redundant_ran_result() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-ran".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "cargo test tool_approval".to_string(),
        Vec::new(),
    );
    let before = model.transcript_mut().plain_items();

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Enter.into()),
        "tool approval panel should handle Enter",
    );

    assert_eq!(
        effect,
        Some(AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-ran".to_string(),
            option_id: Some("allow-once".to_string()),
        })
    );
    assert!(
        model.transcript_mut().plain_items() == before,
        "runtime allow should not append a redundant approval result when the tool call item will already show execution"
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        Vec::<(Sender, String)>::new(),
        "tool approval results should not be sent back to the model"
    );
}

#[test]
fn esc_cancels_runtime_permission_without_rejecting() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-cancel".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: Some("reject-always".to_string()),
        },
        "cargo check".to_string(),
        Vec::new(),
    );
    let before = model.transcript_mut().plain_items();

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Esc.into()),
        "tool approval panel should handle Esc",
    );

    assert_eq!(
        effect,
        Some(AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-cancel".to_string(),
            option_id: None,
        })
    );
    assert!(!model.tool_approval_panel_active());
    assert_eq!(
        model.transcript_mut().plain_items(),
        before,
        "Esc is cancellation, so it must not append a reject result"
    );
}

#[test]
fn unavailable_runtime_approval_choice_is_handled_without_response() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-missing-choice".to_string(),
            allow_option_id: None,
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "cargo check".to_string(),
        Vec::new(),
    );

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Char('y').into()),
        "missing allow shortcut should still be consumed by the modal panel",
    );

    assert_eq!(effect, None);
    assert!(
        model.tool_approval_panel_active(),
        "missing runtime option must not close the approval panel or respond with an empty option id"
    );
}

#[test]
fn stale_runtime_selection_is_handled_without_defaulting_to_deny() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-stale-selection".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: None,
            reject_always_option_id: None,
        },
        "cargo check".to_string(),
        Vec::new(),
    );
    model.tool_approval_panel.selected = 99;

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyCode::Enter.into()),
        "stale selection should still be consumed by the modal panel",
    );

    assert_eq!(effect, None);
    assert!(
        model.tool_approval_panel_active(),
        "stale selection must not fall back to a different runtime approval choice"
    );
}

#[test]
fn file_preview_panel_renders_added_diff_without_transport_json() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: TEMP.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "TEMP.md".to_string(),
            "# 临时文档\n\nbody\n  indented".to_string(),
        )),
    );

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let text = lines.join("\n");

    assert!(
        !text.contains("Create file") && !text.contains("Edit file"),
        "file preview should keep the header to the diff summary only: {lines:?}"
    );
    assert!(
        text.contains("TEMP.md"),
        "preview path should render: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| line.trim() != "TEMP.md"),
        "file preview should not render a standalone path row before the diff summary: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .position(|line| line.contains("Added TEMP.md (+4 -0)"))
            < lines
                .iter()
                .position(|line| line == "      1 +  # 临时文档"),
        "diff summary should act as the inline content header: {lines:?}"
    );
    assert!(
        text.contains("Added TEMP.md (+4 -0)")
            && lines.iter().any(|line| line == "      1 +  # 临时文档")
            && lines.iter().any(|line| line == "      2 +  ")
            && lines.iter().any(|line| line == "      3 +  body")
            && lines.iter().any(|line| line == "      4 +    indented"),
        "file preview should render added diff content: {lines:?}"
    );
    assert!(
        !text.contains("\"path\"") && !text.contains("\"content\""),
        "file preview should not expose raw transport JSON: {lines:?}"
    );
    assert!(
        text.contains("Do you want to create TEMP.md?")
            && text.contains("y/Enter approve")
            && text.contains("n reject")
            && text.contains("Esc cancel"),
        "file preview should use the single approval command bar: {lines:?}"
    );
    assert!(
        !text.contains("1. Yes")
            && !text.contains("Yes, allow all edits during this session")
            && !text.contains("PgUp/PgDn"),
        "inline file preview should not render vertical choices or fullscreen scroll hints: {lines:?}"
    );
}

#[test]
fn inline_file_preview_expands_diff_without_transcript_hint_when_it_fits() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(72, 80);
    model.palette = default_palette();
    let content = (1..=12)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );

    let panel = model.current_inline_tool_approval_panel_render_result();
    let text = panel.plain_lines.join("\n");

    assert!(panel.has_content, "fitting diff should stay inline");
    assert!(
        text.contains("     12 +  line 12"),
        "inline preview should show the full fitting diff: {text:?}"
    );
    assert!(
        !text.contains("ctrl + t to view transcript"),
        "approval preview should not point at transcript overlay: {text:?}"
    );
    assert!(
        text.contains("● Added temp.md (+12 -0)")
            && text.contains("Do you want to create temp.md?")
            && text.contains("y/Enter approve")
            && text.contains("n reject"),
        "inline preview should share the fullscreen-style header and command bar: {text:?}"
    );
    assert!(
        !text.contains("1. Yes") && !text.contains("PgUp/PgDn"),
        "inline preview should not expose the old choice picker or fullscreen scroll controls: {text:?}"
    );
}

#[test]
fn overflowing_file_preview_uses_fullscreen_instead_of_inline_panel() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(72, 12);
    model.palette = default_palette();
    let content = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );

    let panel = model.current_inline_tool_approval_panel_render_result();

    assert!(
        !panel.has_content,
        "overflowing file preview should leave document flow for fullscreen review"
    );
    assert!(
        model.tool_approval_panel_active(),
        "approval state must remain open while fullscreen preview is active"
    );
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "fullscreen preview still starts on the default approval choice"
    );

    let _ = model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "Down should scroll fullscreen diff, not move the approval choice"
    );
}

#[test]
fn fullscreen_file_preview_renders_scrollable_diff_with_approval_footer() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(120, 12);
    model.set_palette(default_palette(), true);
    let content = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );

    let initial = rendered_model_rows(&mut model, 120, 12).join("\n");
    assert!(
        initial.contains("● Added temp.md (+30 -0)"),
        "fixed diff summary should render in the first row: {initial:?}"
    );
    assert!(
        initial.contains("line 1") && !initial.contains("line 30"),
        "fullscreen preview should start at the top of the diff: {initial:?}"
    );
    assert!(
        initial.contains("Do you want to create temp.md?")
            && initial.contains("y/Enter approve")
            && initial.contains("n reject")
            && initial.contains("PgUp/PgDn"),
        "fullscreen preview should show a single command-bar footer: {initial:?}"
    );
    assert!(
        initial.contains("0%") && initial.contains("──"),
        "fullscreen preview should keep the fixed progress divider above the command bar: {initial:?}"
    );
    assert!(
        !initial.contains("1. Yes") && !initial.contains("←→ choice"),
        "fullscreen preview should not render the vertical choice picker: {initial:?}"
    );

    let _ = model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::End));
    let bottom = rendered_model_rows(&mut model, 120, 12).join("\n");

    assert!(
        bottom.contains("● Added temp.md (+30 -0)"),
        "diff summary should stay fixed while scrolled: {bottom:?}"
    );
    assert!(
        bottom.contains("line 30"),
        "End should jump to the bottom of the full diff: {bottom:?}"
    );
    assert!(
        bottom.contains("Do you want to create temp.md?"),
        "approval command bar should remain visible while scrolled: {bottom:?}"
    );
    assert!(
        bottom.contains("100%") && bottom.contains("──"),
        "fullscreen preview should update the progress divider while scrolled: {bottom:?}"
    );
}

#[test]
fn fullscreen_file_preview_uses_direct_approval_keys_without_choice_navigation() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(48, 12);
    model.set_palette(default_palette(), true);
    let content = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );

    let _ = model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Right));
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "fullscreen mode should not use left/right choice navigation"
    );

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter)),
        "fullscreen key should be handled",
    )
    .expect("Enter should approve the preview");

    assert_eq!(
        effect,
        AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            option_id: Some("allow-once".to_string()),
        }
    );
}

#[test]
fn fullscreen_file_preview_uses_overlay_mouse_policy_and_mouse_wheel_scroll() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(48, 12);
    model.set_palette(default_palette(), true);
    let content = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: temp.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "temp.md".to_string(),
            content,
        )),
    );

    assert!(
        !model.wants_mouse_capture(),
        "fullscreen preview should use overlay mouse policy so wheel maps to pager navigation"
    );
    assert_eq!(
        model
            .tool_approval_panel
            .file_preview
            .as_ref()
            .expect("file preview state should exist")
            .scroll_offset,
        0
    );

    model.update(AppEvent::MouseWheel { delta_lines: 3 });

    assert_eq!(
        model
            .tool_approval_panel
            .file_preview
            .as_ref()
            .expect("file preview state should exist")
            .scroll_offset,
        3,
        "mouse wheel events should scroll the fullscreen diff if delivered directly"
    );

    model.close_tool_approval_panel();
    assert!(
        model.wants_mouse_capture(),
        "closing fullscreen approval should restore normal mouse capture"
    );
}

#[test]
fn raw_input_without_structured_diff_does_not_create_file_preview() {
    let existing_path = std::env::current_exe()
        .expect("the running test executable should have a filesystem path")
        .to_string_lossy()
        .into_owned();
    let update = runtime_domain::session::RuntimeToolActivityUpdate {
        activity_id: "call-edit-without-preview".to_string(),
        title: Some(format!("Edit {existing_path}")),
        kind: Some(runtime_domain::session::RuntimeToolKind::Edit),
        status: Some(runtime_domain::session::RuntimeToolActivityStatus::Pending),
        content: Some(vec![
            runtime_domain::session::RuntimeToolActivityContent::Text(
                "Requesting approval".to_string(),
            ),
        ]),
        raw_input: Some(
            serde_json::json!({
                "path": existing_path,
                "content": "replacement content"
            })
            .into(),
        ),
        ..runtime_domain::session::RuntimeToolActivityUpdate::default()
    };

    assert!(
        ToolApprovalPreview::from_runtime_tool_activity_update(&update).is_none(),
        "raw input cannot provide the old text required for a truthful file diff"
    );
}

#[test]
fn edit_preview_panel_renders_diff_instead_of_new_file_snapshot() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    let update = runtime_domain::session::RuntimeToolActivityUpdate {
        activity_id: "call-edit".to_string(),
        title: Some("Edit temp.md".to_string()),
        kind: Some(runtime_domain::session::RuntimeToolKind::Edit),
        status: Some(runtime_domain::session::RuntimeToolActivityStatus::Pending),
        content: Some(vec![
            runtime_domain::session::RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("1. 第一项\n2. 第二项\n3. 第三项\n".to_string()),
                new_text: "1. 第一项\n3. 第三项\n".to_string(),
                is_truncated: false,
            },
        ]),
        ..runtime_domain::session::RuntimeToolActivityUpdate::default()
    };
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-edit".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Edit temp.md".to_string(),
        Vec::new(),
        ToolApprovalPreview::from_runtime_tool_activity_update(&update),
    );

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let text = lines.join("\n");

    assert!(
        text.contains("Edited temp.md (+0 -1)"),
        "edit preview should render the diff summary: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line == "      2 -  2. 第二项"),
        "edit preview should show deleted content instead of only the final file: {lines:?}"
    );
    assert!(
        !text.contains("      2  3. 第三项"),
        "edit preview should not render only numbered new file content: {lines:?}"
    );
}

#[test]
fn existing_empty_file_preview_preserves_edit_semantics() {
    let update = runtime_domain::session::RuntimeToolActivityUpdate {
        activity_id: "call-edit-empty".to_string(),
        title: Some("Edit empty.rs".to_string()),
        kind: Some(runtime_domain::session::RuntimeToolKind::Edit),
        status: Some(runtime_domain::session::RuntimeToolActivityStatus::Pending),
        content: Some(vec![
            runtime_domain::session::RuntimeToolActivityContent::Diff {
                path: "empty.rs".to_string(),
                old_text: Some(String::new()),
                new_text: "fn main() {}\n".to_string(),
                is_truncated: false,
            },
        ]),
        ..runtime_domain::session::RuntimeToolActivityUpdate::default()
    };
    let preview = ToolApprovalPreview::from_runtime_tool_activity_update(&update)
        .expect("Diff content should create an approval preview");

    assert_eq!(preview.question(), "Do you want to edit empty.rs?");
    assert_eq!(preview.old_text(), Some(""));

    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-edit-empty".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Edit empty.rs".to_string(),
        Vec::new(),
        Some(preview),
    );

    let text = build_panel_lines(&mut model, 72)
        .into_iter()
        .flat_map(|line| line.spans)
        .map(|span| span.content.into_owned())
        .collect::<String>();

    assert!(
        text.contains("Edited empty.rs (+1 -0)"),
        "existing empty files must keep the edited header: {text:?}"
    );
}

#[test]
fn file_preview_reuses_item_within_revision_and_rebuilds_for_next_revision() {
    reset_file_preview_item_build_count();
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(72, 80);
    model.set_palette(default_palette(), true);
    let update = runtime_domain::session::RuntimeToolActivityUpdate {
        activity_id: "call-edit-cache".to_string(),
        title: Some("Edit cached.rs".to_string()),
        kind: Some(runtime_domain::session::RuntimeToolKind::Edit),
        status: Some(runtime_domain::session::RuntimeToolActivityStatus::Pending),
        content: Some(vec![
            runtime_domain::session::RuntimeToolActivityContent::Diff {
                path: "cached.rs".to_string(),
                old_text: Some("let value = 1;\n".to_string()),
                new_text: "let value = 2;\n".to_string(),
                is_truncated: false,
            },
        ]),
        ..runtime_domain::session::RuntimeToolActivityUpdate::default()
    };

    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-edit-cache".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Edit cached.rs".to_string(),
        Vec::new(),
        ToolApprovalPreview::from_runtime_tool_activity_update(&update),
    );

    let _ = build_panel_lines(&mut model, 72);
    let _ = build_panel_lines(&mut model, 72);
    model.set_window(64, 80);
    let _ = build_panel_lines(&mut model, 64);
    model.set_palette(terminal_default_palette(), true);
    let _ = build_panel_lines(&mut model, 64);

    assert_eq!(
        file_preview_item_build_count(),
        1,
        "one preview revision must own one reusable ToolResultItem"
    );

    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write-next".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Write next.rs".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "next.rs",
            "fn next() {}\n",
        )),
    );

    assert_eq!(
        file_preview_item_build_count(),
        2,
        "a new preview revision must replace the previous ToolResultItem"
    );
}

#[test]
fn edit_preview_panel_marks_truncated_diff_as_partial() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    let update = runtime_domain::session::RuntimeToolActivityUpdate {
        activity_id: "call-edit".to_string(),
        title: Some("Edit temp.md".to_string()),
        kind: Some(runtime_domain::session::RuntimeToolKind::Edit),
        status: Some(runtime_domain::session::RuntimeToolActivityStatus::Pending),
        content: Some(vec![
            runtime_domain::session::RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: true,
            },
        ]),
        ..runtime_domain::session::RuntimeToolActivityUpdate::default()
    };
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-edit".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: None,
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "Edit temp.md".to_string(),
        Vec::new(),
        ToolApprovalPreview::from_runtime_tool_activity_update(&update),
    );

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(
        lines.iter().any(|line| line.contains("preview truncated")),
        "truncated approval diffs should clearly say the preview is partial: {lines:?}"
    );
}

#[test]
fn file_preview_panel_separates_question_from_direct_key_hints_without_choice_picker() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: TEMP.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "TEMP.md".to_string(),
            "body".to_string(),
        )),
    );

    let lines = build_panel_lines(&mut model, 72)
        .into_iter()
        .map(|line| {
            line.spans
                .into_iter()
                .map(|span| span.content.into_owned())
                .collect::<String>()
        })
        .collect::<Vec<_>>();
    let text = lines.join("\n");

    assert_strictly_ordered_plain_lines(
        &lines,
        &[
            "Do you want to create TEMP.md?",
            "y/Enter approve · n reject · Esc cancel",
        ],
    );
    assert!(
        !text.contains("➜ 1. Yes") && !text.contains("2. Yes, allow all edits during this session"),
        "file preview should not render numbered approval choices: {lines:?}"
    );
}

#[test]
fn file_preview_panel_hides_status_notice() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: TEMP.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "TEMP.md".to_string(),
            "body".to_string(),
        )),
    );
    model.show_transient_status_notice("Press Esc again to interrupt");

    assert!(
        !model.current_status_line_render_result().has_content,
        "file preview approval should suppress status notices while waiting for a choice"
    );
}

#[test]
fn inline_file_preview_uses_direct_approval_keys_without_choice_navigation() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: None,
        },
        "WriteFile: TEMP.md".to_string(),
        Vec::new(),
        Some(ToolApprovalPreview::create_file(
            "TEMP.md".to_string(),
            "body".to_string(),
        )),
    );

    assert_eq!(model.tool_approval_panel.selected, 0);
    let _ = model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "inline file preview should not keep hidden vertical choice navigation"
    );
    let _ = model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Right));
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "inline file preview should ignore left/right choice navigation"
    );

    let effect = handled_effect(
        model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Enter)),
        "inline file preview key should be handled",
    )
    .expect("Enter should approve the preview");

    assert_eq!(
        effect,
        AppEffect::RespondRuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-write".to_string(),
            option_id: Some("allow-once".to_string()),
        }
    );
}

#[test]
fn arrow_keys_move_linearly_between_vertical_choices() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::RuntimePermission {
            target: runtime_domain::session::RuntimeTarget::provider("local", "qwen3"),
            request_id: "permission-3".to_string(),
            allow_option_id: Some("allow-once".to_string()),
            allow_always_option_id: Some("allow-always".to_string()),
            reject_option_id: Some("reject-once".to_string()),
            reject_always_option_id: Some("reject-always".to_string()),
        },
        "touch src/main.rs".to_string(),
        Vec::new(),
    );

    let _ = model.handle_tool_approval_panel_key(KeyCode::Down.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::AllowInSession)
    );

    let _ = model.handle_tool_approval_panel_key(KeyCode::Down.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::Deny)
    );

    let _ = model.handle_tool_approval_panel_key(KeyCode::Right.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::DenyInSession)
    );

    let _ = model.handle_tool_approval_panel_key(KeyCode::Up.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::Deny)
    );
}

fn selected_tool_approval_choice(model: &Model) -> Option<ToolApprovalChoice> {
    tool_approval_choices(&model.tool_approval_panel)
        .get(model.tool_approval_panel.selected)
        .copied()
}

fn open_preview_panel(model: &mut Model) {
    model.open_tool_approval_panel(
        ToolApprovalSource::Preview,
        "sed -n '1,80p' src/main.rs".to_string(),
        Vec::new(),
    );
}

#[test]
fn shell_command_lines_use_highlighted_styles() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let command_line = build_panel_lines(&mut model, 72)
        .into_iter()
        .find(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            text.contains("sed -n")
        })
        .expect("command line should render");
    let foregrounds = command_line
        .spans
        .iter()
        .filter_map(|span| span.style.fg)
        .fold(Vec::new(), |mut colors, color| {
            if !colors.contains(&color) {
                colors.push(color);
            }
            colors
        });

    assert!(
        foregrounds.len() > 1,
        "shell command should have syntax-highlighted spans, got: {command_line:?}"
    );
}

#[test]
fn terminal_default_approval_command_does_not_emit_syntect_rgb_foregrounds() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = terminal_default_palette();
    open_preview_panel(&mut model);

    let command_line = build_panel_lines(&mut model, 72)
        .into_iter()
        .find(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .contains("sed -n")
        })
        .expect("command line should render");

    assert!(
        command_line
            .spans
            .iter()
            .all(|span| { !matches!(span.style.fg, Some(ratatui::style::Color::Rgb(_, _, _))) })
    );
}

fn assert_ordered_plain_lines(lines: &[String], needles: &[&str]) {
    let mut last_index = None;
    for needle in needles {
        let index = lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("expected {needle:?} in {lines:?}"));
        if let Some(last_index) = last_index {
            assert!(
                index >= last_index,
                "expected {needle:?} after previous item in {lines:?}"
            );
        }
        last_index = Some(index);
    }
}

fn assert_strictly_ordered_plain_lines(lines: &[String], needles: &[&str]) {
    let mut previous_index = None;
    for needle in needles {
        let index = lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("expected {needle:?} in {lines:?}"));
        if let Some(previous_index) = previous_index {
            assert!(
                index > previous_index,
                "expected {needle:?} on a later line in {lines:?}"
            );
        }
        previous_index = Some(index);
    }
}

fn rendered_model_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    buffer_rows(&rendered_model_buffer(model, width, height))
}

fn rendered_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    (0..buffer.area.height)
        .map(|row| {
            let mut line = String::new();
            for column in 0..buffer.area.width {
                line.push_str(buffer[(column, row)].symbol());
            }
            line
        })
        .collect()
}
