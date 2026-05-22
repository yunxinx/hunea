use super::*;
use crate::{
    HeroOptions, Sender,
    theme::{default_palette, primary_text_style, secondary_text_style},
};

#[test]
fn preview_layout_omits_labels_and_uses_vertical_numbered_choices() {
    let mut model = Model::new(HeroOptions::default());
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
        "selected choice should match the file-preview marker and numbering style: {lines:?}"
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
    let mut model = Model::new(HeroOptions::default());
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
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.set_window(24, 8);
    model.open_tool_approval_panel(
        ToolApprovalSource::Preview,
        "cargo run --bin lumos -- --very-long-debug-command-that-wraps".to_string(),
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
fn acp_session_allow_option_only_renders_when_available() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::AcpPermission {
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
        ToolApprovalSource::AcpPermission {
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
fn choices_render_vertically_like_file_preview_panel() {
    let mut model = Model::new(HeroOptions::default());
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
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    open_preview_panel(&mut model);

    let effect = model
        .handle_tool_approval_panel_key(KeyCode::Enter.into())
        .expect("tool approval panel should handle Enter");

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
fn acp_allow_choice_does_not_append_redundant_ran_result() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::AcpPermission {
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

    let effect = model
        .handle_tool_approval_panel_key(KeyCode::Enter.into())
        .expect("tool approval panel should handle Enter");

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-ran".to_string(),
            option_id: Some("allow-once".to_string()),
            is_rejection: false,
            rejected_tool_call_id: None,
        })
    );
    assert!(
        model.transcript_mut().plain_items() == before,
        "ACP allow should not append a redundant approval result when the tool call item will already show execution"
    );
    assert_eq!(
        model.transcript_mut().source_messages(),
        Vec::<(Sender, String)>::new(),
        "tool approval results should not be sent back to the model"
    );
}

#[test]
fn esc_cancels_acp_permission_without_rejecting() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::AcpPermission {
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

    let effect = model
        .handle_tool_approval_panel_key(KeyCode::Esc.into())
        .expect("tool approval panel should handle Esc");

    assert_eq!(
        effect,
        Some(AppEffect::RespondAcpPermission {
            request_id: "permission-cancel".to_string(),
            option_id: None,
            is_rejection: false,
            rejected_tool_call_id: None,
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
fn file_preview_panel_renders_numbered_content_without_transport_json() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::AcpPermission {
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
        "file preview should keep the header to the file path only: {lines:?}"
    );
    assert!(
        text.contains("TEMP.md"),
        "preview path should render: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line == "      1  # 临时文档")
            && lines.iter().any(|line| line == "      2  ")
            && lines.iter().any(|line| line == "      3  body")
            && lines.iter().any(|line| line == "      4    indented"),
        "file preview should render numbered file content: {lines:?}"
    );
    assert!(
        !text.contains("\"path\"") && !text.contains("\"content\""),
        "file preview should not expose raw transport JSON: {lines:?}"
    );
    assert!(
        text.contains("Yes") && text.contains("Yes, allow all edits during this session"),
        "file preview should use user-facing approval labels: {lines:?}"
    );
}

#[test]
fn file_preview_panel_choices_use_model_panel_selection_style() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::AcpPermission {
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

    let selected_line = build_panel_lines(&model, 72)
        .into_iter()
        .find(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .contains("1. Yes")
        })
        .expect("selected file preview choice should render");
    let plain = selected_line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(plain, "  ➜ 1. Yes");
    assert_eq!(selected_line.spans[1].content.as_ref(), "➜ ");
    assert_eq!(
        selected_line.spans[1].style,
        secondary_text_style(model.palette)
    );
    assert_eq!(
        selected_line.spans[2].style,
        primary_text_style(model.palette).bold()
    );
}

#[test]
fn file_preview_panel_hides_status_notice() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::AcpPermission {
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
fn file_preview_panel_selection_moves_linearly_for_vertical_choices() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::AcpPermission {
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
        model.tool_approval_panel.selected, 1,
        "vertical preview choices should move from Yes to session allow with Down"
    );
    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        model.tool_approval_panel.selected, 2,
        "vertical preview choices should then move to No"
    );
    model.handle_tool_approval_panel_key(KeyEvent::from(KeyCode::Up));
    assert_eq!(model.tool_approval_panel.selected, 1);
}

#[test]
fn arrow_keys_move_linearly_between_vertical_choices() {
    let mut model = Model::new(HeroOptions::default());
    model.palette = default_palette();
    model.open_tool_approval_panel(
        ToolApprovalSource::AcpPermission {
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
    let mut model = Model::new(HeroOptions::default());
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
