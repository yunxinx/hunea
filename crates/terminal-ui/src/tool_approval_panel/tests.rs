use super::*;
use crate::{
    AppEvent, Sender, StartupBannerOptions, overlay_key_result::OverlayKeyResult,
    theme::default_palette,
};
use ratatui::{buffer::Buffer, layout::Rect};

fn handled_effect(result: OverlayKeyResult, context: &str) -> Option<AppEffect> {
    assert!(!result.is_ignored(), "{context}");
    result.into_effect()
}

#[test]
fn preview_layout_omits_labels_and_uses_vertical_numbered_choices() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let lines = build_panel_lines(&model, 72)
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

    let lines = build_panel_lines(&model, 28)
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

    let without_session = build_panel_lines(&model, 72)
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

    let with_session = build_panel_lines(&model, 72)
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
fn choices_render_vertically_for_command_approval() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let lines = build_panel_lines(&model, 72)
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

    let lines = build_panel_lines(&model, 72)
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

    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
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

    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::End));
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

    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Right));
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
    assert_eq!(model.tool_approval_panel.preview_scroll_offset, 0);

    model.update(AppEvent::MouseWheel { delta_lines: 3 });

    assert_eq!(
        model.tool_approval_panel.preview_scroll_offset, 3,
        "mouse wheel events should scroll the fullscreen diff if delivered directly"
    );

    model.close_tool_approval_panel();
    assert!(
        model.wants_mouse_capture(),
        "closing fullscreen approval should restore normal mouse capture"
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

    let lines = build_panel_lines(&model, 72)
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

    let lines = build_panel_lines(&model, 72)
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
fn file_preview_panel_uses_single_command_bar_without_choice_picker() {
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

    let lines = build_panel_lines(&model, 72)
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
        text.contains("Do you want to create TEMP.md?  y/Enter approve · n reject · Esc cancel"),
        "file preview should render one command bar footer: {lines:?}"
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
    model.show_transient_status_notice("Selection copied");

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
    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        model.tool_approval_panel.selected, 0,
        "inline file preview should not keep hidden vertical choice navigation"
    );
    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Right));
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

    model.handle_tool_approval_panel_key(KeyCode::Down.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::AllowInSession)
    );

    model.handle_tool_approval_panel_key(KeyCode::Down.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::Deny)
    );

    model.handle_tool_approval_panel_key(KeyCode::Right.into());
    assert_eq!(
        selected_tool_approval_choice(&model),
        Some(ToolApprovalChoice::DenyInSession)
    );

    model.handle_tool_approval_panel_key(KeyCode::Up.into());
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

    let command_line = build_panel_lines(&model, 72)
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

fn rendered_model_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
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
