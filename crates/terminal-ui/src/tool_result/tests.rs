use ratatui::style::Modifier;

use runtime_domain::session::RuntimeToolKind;

use super::*;
use crate::{
    styled_text::line_to_plain_text,
    theme::{default_palette, terminal_default_palette},
};

#[test]
fn ran_result_uses_quote_color_without_italic() {
    let palette = default_palette();
    let item = ToolResultItem::new("Ran Write file", ToolResultKind::Ran);
    let lines = item.render_lines(80, palette);

    assert_eq!(line_to_plain_text(&lines[0]), "● Write file");
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
    assert_eq!(lines[0].spans[1].content.as_ref(), "Write");
    assert!(lines[0].spans[1].style.fg.is_none());
    assert!(
        lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(lines[0].spans[2].style.fg.is_none());
    assert!(
        !lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::ITALIC)
    );
    assert!(
        !lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::ITALIC)
    );
}

#[test]
fn rejected_result_uses_approval_rejected_color() {
    let palette = default_palette();
    let item = ToolResultItem::new("Reject Run destructive command", ToolResultKind::Rejected);
    let lines = item.render_lines(80, palette);

    assert_eq!(
        line_to_plain_text(&lines[0]),
        "● Reject destructive command"
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.approval_rejected));
    assert_eq!(lines[0].spans[1].content.as_ref(), "Reject");
    assert!(lines[0].spans[1].style.fg.is_none());
    assert!(
        lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(lines[0].spans[2].style.fg.is_none());
}

#[test]
fn rejected_non_shell_result_preserves_non_run_title_action() {
    let item = ToolResultItem::new("Reject Write file", ToolResultKind::Rejected);
    let lines = item.render_lines(80, default_palette());

    assert_eq!(line_to_plain_text(&lines[0]), "● Reject Write file");
}

#[test]
fn shell_result_removes_shell_prefix_and_highlights_command() {
    let palette = default_palette();
    let item = ToolResultItem::new("Ran Shell: cat Cargo.toml", ToolResultKind::Ran);
    let lines = item.render_lines(80, palette);

    assert_eq!(line_to_plain_text(&lines[0]), "● Ran cat Cargo.toml");
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
    assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
    assert!(lines[0].spans[1].style.fg.is_none());
    assert!(
        lines[0].spans[1]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(2)
            .any(|span| span.style.fg.is_some()),
        "shell command spans should carry syntax highlight foreground colors: {:?}",
        lines[0].spans
    );
}

#[test]
fn terminal_default_shell_result_does_not_emit_syntect_rgb_foregrounds() {
    let item = ToolResultItem::new("Ran Shell: cat Cargo.toml", ToolResultKind::Ran);
    let lines = item.render_lines(80, terminal_default_palette());

    assert!(
        lines[0]
            .spans
            .iter()
            .skip(2)
            .all(|span| { !matches!(span.style.fg, Some(ratatui::style::Color::Rgb(_, _, _))) })
    );
}

#[test]
fn runtime_tool_activity_header_uses_title_only_and_strips_shell_prefix() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(80, palette);

    assert_eq!(line_to_plain_text(&lines[0]), "● Ran cargo check");
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("Completed")),
        "status text should not be part of the runtime header: {:?}",
        lines[0].spans
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("[Other]")),
        "kind label should not be part of the runtime header: {:?}",
        lines[0].spans
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("Shell:")),
        "tool prefix should be stripped from the runtime header: {:?}",
        lines[0].spans
    );
}

#[test]
fn runtime_tool_activity_header_highlights_shell_titles() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(80, palette);

    assert_eq!(line_to_plain_text(&lines[0]), "● Ran cargo check");
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.fg.is_some()),
        "shell-like runtime titles should carry syntax highlight foreground colors: {:?}",
        lines[0].spans
    );
}

#[test]
fn completed_execute_tool_call_uses_raw_command_when_shell_label_has_no_colon() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Shell git commit -m \"feat: demo\"".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"git commit -m \"feat: demo\""}"#.into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(80, palette);

    assert_eq!(
        line_to_plain_text(&lines[0]),
        "● Ran git commit -m \"feat: demo\""
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("Shell")),
        "shell label should not be part of the runtime header: {:?}",
        lines[0].spans
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.fg.is_some()),
        "raw shell command should carry syntax highlight foreground colors: {:?}",
        lines[0].spans
    );
}

#[test]
fn pending_execute_tool_call_uses_raw_command_when_shell_label_has_no_colon() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-approval".to_string(),
            title: "Shell cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Pending,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"cargo check"}"#.into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec!["● cargo check".to_string(), "  └ Waiting...".to_string()]
    );
}

#[test]
fn pending_execute_tool_call_renders_waiting_detail() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-approval".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Pending,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec!["● cargo check".to_string(), "  └ Waiting...".to_string()]
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("Requesting approval")),
        "tool call row should not duplicate the approval panel request text: {rendered_plain:?}"
    );
    assert_eq!(
        lines[1].spans[0].style.fg,
        Some(palette.tertiary),
        "waiting detail should use the same muted branch prefix as exploration rows"
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "waiting detail text should remain visually weak: {:?}",
        lines[1].spans
    );
}

#[test]
fn active_execute_tool_call_defers_streamed_content_until_finished() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::InProgress,
            content: vec![RuntimeToolActivityContent::Text(
                "Requesting approval to perform: Run command `cargo check`".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"cargo check"}"#.into()),
            raw_output: Some("Checking hunea v0.1.0".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec!["● cargo check".to_string(), "  └ Waiting...".to_string()]
    );
    assert!(
        rendered_plain.iter().all(|line| {
            !line.contains("Requesting approval")
                && !line.contains("Checking hunea")
                && !line.contains(r#"{"command":"cargo check"}"#)
        }),
        "active command tool calls should not stream command details in the main transcript: {rendered_plain:?}"
    );
}

#[test]
fn completed_execute_tool_call_renders_deferred_content() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some("Checking hunea v0.1.0".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran cargo check".to_string(),
            "  └ Checking hunea v0.1.0".to_string(),
        ]
    );
}

#[test]
fn completed_execute_tool_call_strips_legacy_run_prefix_after_ran_header() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Run tests".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some("ok".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Ran tests");
}

#[test]
fn completed_execute_tool_call_compact_output_keeps_two_head_and_tail_lines() {
    let raw_output = (1..=11)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: ls".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(raw_output.into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran ls".to_string(),
            "  └ line 1".to_string(),
            "    line 2".to_string(),
            "    … +7 lines (ctrl + t to view transcript)".to_string(),
            "    line 10".to_string(),
            "    line 11".to_string(),
        ]
    );
}

#[test]
fn completed_execute_tool_call_compact_output_wraps_at_spaces() {
    let raw_output = (1..=70)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: seq 1 70".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(raw_output.into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[1].split_whitespace().last(), Some("28"));
    assert!(
        rendered_plain[2].starts_with("    29 "),
        "fixture should resume at the next complete number: {rendered_plain:?}"
    );
}

#[test]
fn completed_execute_tool_call_uses_display_content_without_model_truncation_footer() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: seq 1000".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    concat!(
                        "line 999\n",
                        "line 1000\n\n",
                        "[Showing lines 999-1000 of 1000. Full output: /tmp/hunea-bash.log]"
                    ),
                    Some("line 999\nline 1000"),
                    Some(serde_json::json!({
                        "truncated": true,
                        "full_output_path": "/tmp/hunea-bash.log"
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran seq 1000".to_string(),
            "  └ line 999".to_string(),
            "    line 1000".to_string(),
        ]
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("[Showing lines")),
        "model-visible truncation footer should not be rendered as shell output: {rendered_plain:?}"
    );
}

#[test]
fn detailed_execute_tool_call_uses_display_content_without_model_truncation_footer() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: seq 1000".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    concat!(
                        "line 999\n",
                        "line 1000\n\n",
                        "[Showing lines 999-1000 of 1000. Full output: /tmp/hunea-bash.log]"
                    ),
                    Some("line 999\nline 1000"),
                    Some(serde_json::json!({
                        "exit_code": 0,
                        "duration_ms": 1500,
                        "truncated": true,
                        "full_output_path": "/tmp/hunea-bash.log"
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Detailed,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "$ seq 1000".to_string(),
            "line 999".to_string(),
            "line 1000".to_string(),
            "".to_string(),
            "✓ • 1.50s".to_string(),
        ]
    );
}

#[test]
fn detailed_execute_tool_call_appends_duration_footer() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result(
                    "Finished dev profile",
                    Some(serde_json::json!({
                        "exit_code": 0,
                        "duration_ms": 1500
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Detailed,
    );
    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "$ cargo check".to_string(),
            "Finished dev profile".to_string(),
            "".to_string(),
            "✓ • 1.50s".to_string(),
        ]
    );
    assert!(
        lines[1].spans.iter().all(|span| span.style.fg.is_none()),
        "detailed output should use the terminal default color: {:?}",
        lines[1].spans
    );
    assert_eq!(lines[3].spans[0].content.as_ref(), "✓");
    assert_eq!(lines[3].spans[0].style.fg, Some(palette.quote));
    assert!(
        lines[3]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "footer duration text should stay visually weak: {:?}",
        lines[3].spans
    );
}

#[test]
fn detailed_execute_tool_call_keeps_shell_command_highlighting() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: echo \"hello\"".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"echo \"hello\""}"#.into()),
            raw_output: Some("hello".into()),
        },
        ToolActivityRenderMode::Detailed,
    );
    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec!["$ echo \"hello\"".to_string(), "hello".to_string()]
    );
    assert_eq!(lines[0].spans[0].content.as_ref(), "$ ");
    assert!(lines[0].spans[0].style.fg.is_none());
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.fg.is_some()),
        "detailed command should retain bash syntax highlight colors: {:?}",
        lines[0].spans
    );
    assert!(
        lines[1].spans.iter().all(|span| span.style.fg.is_none()),
        "detailed command output should stay terminal-default colored: {:?}",
        lines[1].spans
    );
}

#[test]
fn detailed_failed_execute_tool_call_appends_exit_footer() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Failed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result(
                    "error: failed",
                    Some(serde_json::json!({
                        "exit_code": 7,
                        "duration_ms": 250
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Detailed,
    );
    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "$ cargo check".to_string(),
            "error: failed".to_string(),
            "".to_string(),
            "✗ (exit 7) • 250ms".to_string(),
        ]
    );
    assert!(
        lines[1].spans.iter().all(|span| span.style.fg.is_none()),
        "detailed output should use the terminal default color: {:?}",
        lines[1].spans
    );
    assert_eq!(lines[3].spans[0].content.as_ref(), "✗");
    assert_eq!(lines[3].spans[0].style.fg, Some(palette.system_error));
    assert!(
        lines[3]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "footer duration text should stay visually weak: {:?}",
        lines[3].spans
    );
}

#[test]
fn completed_execute_tool_call_prefers_raw_output_and_hides_permission_copy_content() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "Requesting approval to perform: Run command `cargo check`".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"cargo check"}"#.into()),
            raw_output: Some("Finished dev profile".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran cargo check".to_string(),
            "  └ Finished dev profile".to_string(),
        ]
    );
    assert!(
        rendered_plain.iter().all(|line| {
            !line.contains("Requesting approval")
                && !line.contains("Input:")
                && !line.contains(r#"{"command":"cargo check"}"#)
        }),
        "completed command rows should show final output without approval copy or raw input: {rendered_plain:?}"
    );
}

#[test]
fn failed_execute_tool_call_renders_final_output_without_raw_input() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Failed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"cargo check"}"#.into()),
            raw_output: Some("error: could not compile `hunea`".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran cargo check".to_string(),
            "  └ error: could not compile `hunea`".to_string(),
        ]
    );
    assert!(
        rendered_plain.iter().all(|line| {
            !line.contains("Input:") && !line.contains(r#"{"command":"cargo check"}"#)
        }),
        "failed command rows should show final output without raw transport input: {rendered_plain:?}"
    );
}

#[test]
fn failed_execute_tool_call_uses_secondary_detail_text_for_rejection_copy() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-exec".to_string(),
            title: "Shell: echo hi".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::Failed,
            content: vec![RuntimeToolActivityContent::Text(
                "Failed: You rejected running this command".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(r#"{"command":"echo hi"}"#.into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );

    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran echo hi".to_string(),
            "  └ Failed: You rejected running this command".to_string(),
        ]
    );
    assert_eq!(lines[1].spans[0].style.fg, Some(palette.tertiary));
    assert!(
        lines[1]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "failed execute detail should use the same weak color as completed command output: {:?}",
        lines[1].spans
    );
}

#[test]
fn failed_file_mutation_tool_call_uses_secondary_detail_text_for_rejection_copy() {
    let palette = default_palette();
    let cases = [
        (
            "call-write",
            "Write temp.md",
            RuntimeToolKind::Write,
            "Failed: You rejected writing this file",
            "● Write temp.md",
        ),
        (
            "call-edit",
            "Edit temp.md",
            RuntimeToolKind::Edit,
            "Failed: You rejected editing this file",
            "● Write temp.md",
        ),
    ];

    for (activity_id, title, kind, detail, header) in cases {
        let item = ToolResultItem::from_runtime_tool_activity(
            RuntimeToolActivity {
                activity_id: activity_id.to_string(),
                title: title.to_string(),
                kind,
                status: RuntimeToolActivityStatus::Failed,
                content: vec![RuntimeToolActivityContent::Text(detail.to_string())],
                locations: Vec::new(),
                raw_input: Some(r#"{"path":"temp.md","content":"body"}"#.into()),
                raw_output: None,
            },
            ToolActivityRenderMode::Compact,
        );

        let lines = item.render_lines(80, palette);
        let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

        assert_eq!(
            rendered_plain,
            vec![header.to_string(), format!("  └ {detail}")]
        );
        assert_eq!(lines[1].spans[0].style.fg, Some(palette.tertiary));
        assert!(
            lines[1]
                .spans
                .iter()
                .skip(1)
                .all(|span| span.style.fg == Some(palette.secondary)),
            "failed file mutation rejection detail should use the same weak color as command rejection output: {:?}",
            lines[1].spans
        );
    }
}

#[test]
fn failed_file_mutation_tool_call_uses_secondary_detail_text_for_runtime_error() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-write".to_string(),
            title: "Write temp.md".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Failed,
            content: vec![RuntimeToolActivityContent::Text(
                "Failed: File must be read before writing".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(r#"{"path":"temp.md","content":"body"}"#.into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );

    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Write temp.md".to_string(),
            "  └ Failed: File must be read before writing".to_string(),
        ]
    );
    assert_eq!(lines[1].spans[0].style.fg, Some(palette.tertiary));
    assert!(
        lines[1]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "failed file mutation runtime error should use weak detail text: {:?}",
        lines[1].spans
    );
}

#[test]
fn completed_non_execute_tool_call_still_renders_text_content() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-fetch".to_string(),
            title: "Fetch package metadata".to_string(),
            kind: RuntimeToolKind::Fetch,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "Found 3 releases".to_string(),
            )],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Fetch package metadata".to_string(),
            "  └ Found 3 releases".to_string(),
        ]
    );
}

#[test]
fn runtime_tool_activity_raw_output_trailing_newline_does_not_render_blank_line() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Shell: cargo check".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some("Checking hunea\n".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered = item.render_lines(80, palette);
    let rendered_plain = rendered.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran cargo check".to_string(),
            "  └ Checking hunea".to_string(),
        ]
    );
    assert!(
        rendered
            .last()
            .is_some_and(|line| !line_to_plain_text(line).trim().is_empty()),
        "rendered runtime output should not end with a blank line: {rendered_plain:?}"
    );
}

#[test]
fn runtime_pending_text_content_is_not_approval_waiting_without_permission_state() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Check policy".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Pending,
            content: vec![RuntimeToolActivityContent::Text(
                "This result requires approval from the project owner.".to_string(),
            )],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("requires approval from the project owner")),
        "plain tool text should remain visible unless the runtime marks the row as waiting for permission: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("Waiting...")),
        "plain tool text must not be inferred as approval waiting from content wording: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_multi_line_raw_output_uses_four_space_continuation_prefix() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Shell: git log --oneline -5".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: None,
            raw_output: Some("first line\nsecond line".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, palette)
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran git log --oneline -5".to_string(),
            "  └ first line".to_string(),
            "    second line".to_string(),
        ]
    );
}

#[test]
fn runtime_tool_activity_terminal_content_renders_live_snapshot() {
    let mut item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-terminal".to_string(),
            title: "Run tests".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::InProgress,
            content: vec![RuntimeToolActivityContent::Terminal {
                terminal_id: "term-1".to_string(),
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    assert!(item.set_runtime_terminal_snapshot_for_test(
        runtime_domain::session::RuntimeTerminalSnapshot {
            terminal_id: "term-1".to_string(),
            command: Some("cargo check".to_string()),
            cwd: None,
            output: "Checking hunea\nFinished".to_string(),
            truncated: false,
            exit_status: None,
            released: false,
        },
    ));

    let plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(plain.contains("Running..."));
    assert!(plain.contains("Checking hunea"));
    assert!(plain.contains("Finished"));
    assert!(!plain.contains("runtime terminal unavailable"));
    assert!(!plain.contains("terminal/create unsupported"));
}

#[test]
fn detailed_runtime_tool_activity_terminal_content_highlights_command_only() {
    let palette = default_palette();
    let mut item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-terminal".to_string(),
            title: "Run tests".to_string(),
            kind: RuntimeToolKind::Execute,
            status: RuntimeToolActivityStatus::InProgress,
            content: vec![RuntimeToolActivityContent::Terminal {
                terminal_id: "term-1".to_string(),
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    assert!(item.set_runtime_terminal_snapshot_for_test(
        runtime_domain::session::RuntimeTerminalSnapshot {
            terminal_id: "term-1".to_string(),
            command: Some("cargo check".to_string()),
            cwd: None,
            output: "Checking hunea\nFinished".to_string(),
            truncated: false,
            exit_status: None,
            released: false,
        },
    ));

    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "$ cargo check".to_string(),
            "Checking hunea".to_string(),
            "Finished".to_string(),
        ]
    );
    assert_eq!(lines[0].spans[0].content.as_ref(), "$ ");
    assert!(lines[0].spans[0].style.fg.is_none());
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.fg.is_some()),
        "detailed live transcript command should retain bash syntax highlight colors: {:?}",
        lines[0].spans
    );
    assert!(
        lines
            .iter()
            .skip(1)
            .all(|line| line.spans.iter().all(|span| span.style.fg.is_none())),
        "detailed live transcript output should use the terminal default color: {lines:?}"
    );
}

#[test]
fn runtime_tool_activity_raw_output_uses_secondary_color_and_codex_like_alignment() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
            RuntimeToolActivity {
                activity_id: "call-1".to_string(),
                title: "Shell: cargo check".to_string(),
                kind: RuntimeToolKind::Other,
                status: RuntimeToolActivityStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: Some(
                    "Checking hunea v0.1.0 (/home/archie/GoCodes/lumos_rust)\nFinished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s"
                        .into(),
                ),
            },
            ToolActivityRenderMode::Compact,
        );
    let lines = item.render_lines(120, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Ran cargo check".to_string(),
            "  └ Checking hunea v0.1.0 (/home/archie/GoCodes/lumos_rust)".to_string(),
            "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s".to_string(),
        ]
    );
    assert!(
        lines[1].spans[0].style.fg == Some(palette.tertiary),
        "raw output prefix should use the muted exploration branch color: {:?}",
        lines[1].spans
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "raw output content should use the secondary semantic color: {:?}",
        lines[1].spans
    );
}

#[test]
fn runtime_read_tool_activity_renders_compact_summary_without_content_details() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "ReadFile: Temp.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "     1  # 临时文件\n     2\n     3  body".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: "Temp.md".to_string(),
                line: Some(1),
            }],
            raw_input: Some(r#"{"path":"Temp.md"}"#.into()),
            raw_output: Some("# 临时文件\nbody".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
}

#[test]
fn runtime_read_tool_activity_appends_completed_partial_line_range() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Read Temp.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "200\tbody\n201\tmore".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: "Temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(
                serde_json::json!({
                    "path": "Temp.md",
                    "offset": 200,
                    "limit": 2
                })
                .into(),
            ),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result(
                    "200\tbody\n201\tmore",
                    Some(serde_json::json!({
                        "path": "Temp.md",
                        "kind": "text",
                        "start_line": 200,
                        "end_line": 201,
                        "total_lines": 500,
                        "next_offset": 202,
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Read Temp.md(200~201)".to_string()]);
}

#[test]
fn runtime_read_tool_activity_keeps_full_file_title_without_line_range() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Read Temp.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("1\tbody".to_string())],
            locations: vec![RuntimeToolActivityLocation {
                path: "Temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "Temp.md" }).into()),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result(
                    "1\tbody",
                    Some(serde_json::json!({
                        "path": "Temp.md",
                        "kind": "text",
                        "start_line": 1,
                        "end_line": 1,
                        "total_lines": 1,
                        "next_offset": null,
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
}

#[test]
fn runtime_read_tool_activity_does_not_infer_line_range_from_input_arguments() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Read Temp.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "200\tbody\n201\tmore".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: "Temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(
                serde_json::json!({
                    "path": "Temp.md",
                    "offset": 200,
                    "limit": 2
                })
                .into(),
            ),
            raw_output: Some("200\tbody\n201\tmore".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
}

#[test]
fn runtime_readfile_title_fallback_renders_compact_summary_even_without_read_kind() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "ReadFile: Temp.md".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "     1  # 临时文件\n     2\n     3  body".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(r#"{"path":"Temp.md"}"#.into()),
            raw_output: Some("# 临时文件\nbody".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Read Temp.md".to_string()]);
}

#[test]
fn list_dir_root_renders_compact_summary_without_content_details() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "Cargo.toml\ncrates/\nsrc/".to_string(),
            )],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "." }).into()),
            raw_output: Some("Cargo.toml\ncrates/\nsrc/".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● List .".to_string()]);
}

#[test]
fn list_dir_subpath_renders_without_dot_slash_prefix() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory ./src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "lib.rs\ntool_result.rs".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: "./src".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "./src" }).into()),
            raw_output: Some("lib.rs\ntool_result.rs".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● List src".to_string()]);
}

#[test]
fn list_dir_absolute_subpath_renders_relative_to_current_dir() {
    let absolute_path = std::env::current_dir()
        .expect("test should run inside the workspace")
        .join("src");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: format!("List Directory {}", absolute_path.display()),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "lib.rs\ntool_result.rs".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: absolute_path.display().to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": absolute_path }).into()),
            raw_output: Some("lib.rs\ntool_result.rs".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● List src".to_string()]);
}

#[test]
fn list_dir_absolute_path_outside_current_dir_shortens_home_prefix() {
    let Some(home_dir) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let absolute_path = home_dir.join("other-project");
    if std::env::current_dir()
        .ok()
        .is_some_and(|cwd| absolute_path.starts_with(cwd))
    {
        return;
    }

    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: format!("List Directory {}", absolute_path.display()),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("README.md".to_string())],
            locations: vec![RuntimeToolActivityLocation {
                path: absolute_path.display().to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": absolute_path }).into()),
            raw_output: Some("README.md".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![format!(
            "● List ~{}other-project",
            std::path::MAIN_SEPARATOR
        )]
    );
}

#[test]
fn list_dir_detailed_mode_keeps_transcript_summary_compact() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text(
                "lib.rs\ntool_result.rs".to_string(),
            )],
            locations: vec![RuntimeToolActivityLocation {
                path: "src".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "src" }).into()),
            raw_output: Some("lib.rs\ntool_result.rs".into()),
        },
        ToolActivityRenderMode::Detailed,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● List src".to_string()]);
}

#[test]
fn list_dir_debug_detailed_mode_expands_raw_input_and_output() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: vec![RuntimeToolActivityLocation {
                path: "src".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "src" }).into()),
            raw_output: Some("lib.rs\ntool_result.rs".into()),
        },
        ToolActivityRenderMode::DebugDetailed,
    );
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("● List src")),
        "debug detailed mode should keep the tool identity visible: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line.contains("Input:"))
            && rendered_plain.iter().any(|line| line.contains("\"path\"")),
        "debug detailed mode should expose raw tool input: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("tool_result.rs")),
        "debug detailed mode should expose raw tool output: {rendered_plain:?}"
    );
}

#[test]
fn exploration_tool_activity_debug_detailed_mode_renders_as_runtime_item() {
    let item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory src".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "src" }).into()),
            raw_output: Some("lib.rs\ntool_result.rs".into()),
        },
        ToolActivityRenderMode::DebugDetailed,
    )
    .expect("debug detailed mode should render exploration activities without grouping");
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("● List src")),
        "debug detailed mode should keep exploration tool identity visible: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line.contains("Input:"))
            && rendered_plain.iter().any(|line| line.contains("\"path\"")),
        "debug detailed mode should expose raw exploration tool input: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("tool_result.rs")),
        "debug detailed mode should expose raw exploration tool output: {rendered_plain:?}"
    );
}

#[test]
fn completed_open_exploration_group_uses_main_marker_color() {
    let palette = default_palette();
    let mut item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory crates".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("tui/".to_string())],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "crates" }).into()),
            raw_output: Some("tui/".into()),
        },
        ToolActivityRenderMode::Compact,
    )
    .expect("list_dir should be an exploration tool activity");
    assert!(item.append_exploration_tool_activity(RuntimeToolActivity {
        activity_id: "call-read".to_string(),
        title: "Read Cargo.toml".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("[package]".to_string())],
        locations: vec![RuntimeToolActivityLocation {
            path: "Cargo.toml".to_string(),
            line: None,
        }],
        raw_input: Some(serde_json::json!({ "path": "Cargo.toml" }).into()),
        raw_output: Some("[package]".into()),
    }));

    let lines = item.render_lines(80, palette);

    assert_eq!(line_to_plain_text(&lines[0]), "● Explored");
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.main));

    assert!(item.mark_exploration_complete());
    let completed_lines = item.render_lines(80, palette);
    assert_eq!(completed_lines[0].spans[0].style.fg, Some(palette.quote));
}

#[test]
fn exploration_group_wraps_target_lists_at_word_boundaries() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        completed_read_call("CLAUDE.md"),
        ToolActivityRenderMode::Compact,
    )
    .expect("read should be an exploration tool activity");

    for path in [
        "config.example.toml",
        "models.example.toml",
        "Cargo.toml",
        "phrases.example.toml",
        ".gitignore",
        "config.example.toml",
    ] {
        assert!(item.append_exploration_tool_activity(completed_read_call(path)));
    }
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(122, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
            rendered_plain,
            vec![
                "● Explored".to_string(),
                "  └ Read CLAUDE.md, config.example.toml, models.example.toml, Cargo.toml, phrases.example.toml, .gitignore,"
                    .to_string(),
                "    config.example.toml".to_string(),
            ]
        );
}

#[test]
fn single_exploration_tool_call_renders_as_standalone_row() {
    let palette = default_palette();
    let mut item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory crates".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("tui/".to_string())],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "crates" }).into()),
            raw_output: Some("tui/".into()),
        },
        ToolActivityRenderMode::Compact,
    )
    .expect("list_dir should be an exploration tool activity");

    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● List crates".to_string()]);
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.main));

    assert!(item.mark_exploration_complete());
    let completed_lines = item.render_lines(80, palette);
    let completed_plain = completed_lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(completed_plain, vec!["● List crates".to_string()]);
    assert_eq!(completed_lines[0].spans[0].style.fg, Some(palette.quote));
}

#[test]
fn single_skill_usage_renders_as_standalone_use_skill_row() {
    let rendered_plain = ToolResultItem::from_exploration_tool_activity(
        completed_skill_usage_call("code-review", false),
        ToolActivityRenderMode::Compact,
    )
    .expect("skill usage should be an exploration-group activity")
    .render_lines(80, default_palette())
    .iter()
    .map(line_to_plain_text)
    .collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Use code-review Skill".to_string()]);
}

#[test]
fn grouped_skill_usage_renders_use_skills_header_and_global_suffix() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        completed_skill_usage_call("repo-bootstrap", false),
        ToolActivityRenderMode::Compact,
    )
    .expect("skill usage should be groupable");
    assert!(
        item.append_exploration_tool_activity(completed_skill_usage_call("code-review", false,))
    );
    assert!(item.append_exploration_tool_activity(completed_skill_usage_call("lint", true,)));
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Use Skills".to_string(),
            "  └ Read repo-bootstrap, code-review, lint(global)".to_string(),
        ]
    );
}

#[test]
fn skill_usage_group_does_not_merge_with_regular_exploration_group() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        completed_skill_usage_call("code-review", false),
        ToolActivityRenderMode::Compact,
    )
    .expect("skill usage should be groupable");

    assert!(
        !item.append_exploration_tool_activity(completed_read_call("Cargo.toml")),
        "skill usage should remain separate from regular exploration groups"
    );
}

#[test]
fn grouped_grep_and_find_keep_specific_actions_and_show_workspace_path() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        completed_grep_call("workspace_relative_path", None),
        ToolActivityRenderMode::Compact,
    )
    .expect("grep should be an exploration tool activity");
    assert!(item.append_exploration_tool_activity(completed_find_call("**/*.rs", Some("."))));
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Explored".to_string(),
            "  └ Grep workspace_relative_path in .".to_string(),
            "    Find **/*.rs in .".to_string(),
        ]
    );
}

#[test]
fn grouped_grep_and_find_coalesce_consecutive_patterns_by_action_and_path() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        completed_grep_call("hello", None),
        ToolActivityRenderMode::Compact,
    )
    .expect("grep should be an exploration tool activity");
    assert!(item.append_exploration_tool_activity(completed_grep_call("import", Some("."))));
    assert!(item.append_exploration_tool_activity(completed_grep_call("class", Some("crates"))));
    assert!(item.append_exploration_tool_activity(completed_find_call("**/*.toml", None)));
    assert!(item.append_exploration_tool_activity(completed_find_call("**/*.md", Some("."))));
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Explored".to_string(),
            "  └ Grep hello|import in .".to_string(),
            "    Grep class in crates".to_string(),
            "    Find **/*.toml|**/*.md in .".to_string(),
        ]
    );
}

#[test]
fn exploration_group_uses_secondary_style_for_search_and_target_separators() {
    let palette = default_palette();
    let mut search_item = ToolResultItem::from_exploration_tool_activity(
        completed_grep_call("hello", None),
        ToolActivityRenderMode::Compact,
    )
    .expect("grep should be an exploration tool activity");
    assert!(search_item.append_exploration_tool_activity(completed_grep_call("import", Some("."))));
    assert!(search_item.mark_exploration_complete());

    let search_lines = search_item.render_lines(120, palette);
    assert_eq!(
        line_to_plain_text(&search_lines[1]),
        "  └ Grep hello|import in ."
    );
    assert_eq!(
        span_fg(&search_lines[1], "|"),
        Some(palette.secondary),
        "coalesced grep pattern separator should be visually secondary"
    );
    assert_eq!(
        span_fg(&search_lines[1], " in "),
        Some(palette.secondary),
        "grep path connector should be visually secondary"
    );

    let mut read_item = ToolResultItem::from_exploration_tool_activity(
        completed_read_call("Cargo.toml"),
        ToolActivityRenderMode::Compact,
    )
    .expect("read should be an exploration tool activity");
    assert!(read_item.append_exploration_tool_activity(completed_read_call("AGENTS.md")));
    assert!(read_item.mark_exploration_complete());

    let read_lines = read_item.render_lines(120, palette);
    assert_eq!(
        line_to_plain_text(&read_lines[1]),
        "  └ Read Cargo.toml, AGENTS.md"
    );
    assert_eq!(
        span_fg(&read_lines[1], ", "),
        Some(palette.secondary),
        "read target separator should be visually secondary"
    );
}

#[test]
fn standalone_grep_and_find_show_specific_action_without_search_prefix() {
    let palette = default_palette();
    let grep_item = ToolResultItem::from_runtime_tool_activity(
        completed_grep_call("ToolProgress", None),
        ToolActivityRenderMode::Compact,
    );
    let find_item = ToolResultItem::from_runtime_tool_activity(
        completed_find_call("crates/**/*.rs", Some("crates")),
        ToolActivityRenderMode::Compact,
    );

    let grep_lines = grep_item.render_lines(120, palette);
    let find_lines = find_item.render_lines(120, palette);
    let grep_plain = grep_lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();
    let find_plain = find_lines
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(grep_plain, vec!["● Grep ToolProgress in .".to_string()]);
    assert_eq!(
        find_plain,
        vec!["● Find crates/**/*.rs in crates".to_string()]
    );
    assert_eq!(span_fg(&grep_lines[0], " in "), Some(palette.secondary));
    assert_eq!(span_fg(&find_lines[0], " in "), Some(palette.secondary));
}

fn span_fg(line: &ratatui::text::Line<'_>, text: &str) -> Option<ratatui::style::Color> {
    line.spans
        .iter()
        .find(|span| span.content.as_ref() == text)
        .and_then(|span| span.style.fg)
}

#[test]
fn failed_exploration_tool_call_renders_as_standalone_failed_row() {
    let palette = default_palette();
    let mut item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-read".to_string(),
            title: "Read AGENTS.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::InProgress,
            content: Vec::new(),
            locations: vec![RuntimeToolActivityLocation {
                path: "AGENTS.md".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "AGENTS.md" }).into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    )
    .expect("read should be an exploration tool activity");

    assert!(
        item.update_runtime_tool_activity(RuntimeToolActivityUpdate {
            activity_id: "call-read".to_string(),
            status: Some(RuntimeToolActivityStatus::Failed),
            content: Some(vec![RuntimeToolActivityContent::Text(
                "Failed: File not found: AGENTS.md".to_string(),
            )]),
            raw_output: Some("Toolset error: ToolCallError: File not found: AGENTS.md".into(),),
            ..RuntimeToolActivityUpdate::default()
        })
    );

    let lines = item.render_lines(80, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Read AGENTS.md".to_string(),
            "  └ Failed: File not found".to_string(),
        ]
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.system_error));
    assert_eq!(lines[1].spans[0].style.fg, Some(palette.tertiary));
    assert!(
        lines[1]
            .spans
            .iter()
            .skip(1)
            .all(|span| span.style.fg == Some(palette.secondary)),
        "failed reason should use secondary text, not the error color: {:?}",
        lines[1].spans
    );
    assert!(
        rendered_plain.iter().all(|line| {
            !line.contains("Explored")
                && !line.contains("Input:")
                && !line.contains("Toolset error")
                && !line.contains(r#""path""#)
        }),
        "failed exploration rows should not expose grouped detail blocks: {rendered_plain:?}"
    );
}

#[test]
fn failed_exploration_tool_call_is_filtered_from_group_summary() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-cargo".to_string(),
            title: "Read Cargo.toml".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("[package]".to_string())],
            locations: vec![RuntimeToolActivityLocation {
                path: "Cargo.toml".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "Cargo.toml" }).into()),
            raw_output: Some("[package]".into()),
        },
        ToolActivityRenderMode::Compact,
    )
    .expect("read should be an exploration tool activity");
    assert!(item.append_exploration_tool_activity(RuntimeToolActivity {
        activity_id: "call-agents".to_string(),
        title: "Read AGENTS.md".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::InProgress,
        content: Vec::new(),
        locations: vec![RuntimeToolActivityLocation {
            path: "AGENTS.md".to_string(),
            line: None,
        }],
        raw_input: Some(serde_json::json!({ "path": "AGENTS.md" }).into()),
        raw_output: None,
    }));
    assert!(
        item.update_runtime_tool_activity(RuntimeToolActivityUpdate {
            activity_id: "call-agents".to_string(),
            status: Some(RuntimeToolActivityStatus::Failed),
            content: Some(vec![RuntimeToolActivityContent::Text(
                "File not found: AGENTS.md".to_string(),
            )]),
            ..RuntimeToolActivityUpdate::default()
        })
    );
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Explored".to_string(),
            "  └ Read Cargo.toml".to_string(),
            "".to_string(),
            "● Read AGENTS.md".to_string(),
            "  └ Failed: File not found".to_string(),
        ]
    );
}

#[test]
fn failed_exploration_tool_calls_are_separated_inside_one_group() {
    let mut item = ToolResultItem::from_exploration_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-first".to_string(),
            title: "Read first.md".to_string(),
            kind: RuntimeToolKind::Read,
            status: RuntimeToolActivityStatus::InProgress,
            content: Vec::new(),
            locations: vec![RuntimeToolActivityLocation {
                path: "first.md".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({ "path": "first.md" }).into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    )
    .expect("read should be an exploration tool activity");
    assert!(item.append_exploration_tool_activity(RuntimeToolActivity {
        activity_id: "call-second".to_string(),
        title: "Read second.md".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::InProgress,
        content: Vec::new(),
        locations: vec![RuntimeToolActivityLocation {
            path: "second.md".to_string(),
            line: None,
        }],
        raw_input: Some(serde_json::json!({ "path": "second.md" }).into()),
        raw_output: None,
    }));

    for (activity_id, path) in [("call-first", "first.md"), ("call-second", "second.md")] {
        assert!(
            item.update_runtime_tool_activity(RuntimeToolActivityUpdate {
                activity_id: activity_id.to_string(),
                status: Some(RuntimeToolActivityStatus::Failed),
                content: Some(vec![RuntimeToolActivityContent::Text(format!(
                    "File not found: {path}"
                ))]),
                ..RuntimeToolActivityUpdate::default()
            })
        );
    }
    assert!(item.mark_exploration_complete());

    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● Read first.md".to_string(),
            "  └ Failed: File not found".to_string(),
            "".to_string(),
            "● Read second.md".to_string(),
            "  └ Failed: File not found".to_string(),
        ]
    );
}

fn completed_read_call(path: &str) -> RuntimeToolActivity {
    RuntimeToolActivity {
        activity_id: format!("call-read-{path}"),
        title: format!("Read {path}"),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("content".to_string())],
        locations: vec![RuntimeToolActivityLocation {
            path: path.to_string(),
            line: None,
        }],
        raw_input: Some(serde_json::json!({ "path": path }).into()),
        raw_output: Some("content".into()),
    }
}

fn completed_skill_usage_call(skill_name: &str, is_global: bool) -> RuntimeToolActivity {
    let skill_root = if is_global {
        format!("/tmp/home/.agents/skills/{skill_name}")
    } else {
        format!("/tmp/repo/.agents/skills/{skill_name}")
    };
    RuntimeToolActivity {
        activity_id: format!("call-skill-{skill_name}"),
        title: format!("Read {skill_root}/SKILL.md"),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("content".to_string())],
        locations: vec![RuntimeToolActivityLocation {
            path: format!("{skill_root}/SKILL.md"),
            line: None,
        }],
        raw_input: Some(
            serde_json::json!({
                "path": format!("{skill_root}/SKILL.md"),
                "hunea_skill_name": skill_name,
                "hunea_skill_origin": if is_global { "global" } else { "project" },
            })
            .into(),
        ),
        raw_output: Some("content".into()),
    }
}

fn completed_grep_call(pattern: &str, path: Option<&str>) -> RuntimeToolActivity {
    let mut raw_input = serde_json::json!({ "pattern": pattern });
    if let Some(path) = path {
        raw_input["path"] = serde_json::json!(path);
    }

    RuntimeToolActivity {
        activity_id: format!("call-grep-{pattern}"),
        title: "Grep".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text(format!(
            "src/lib.rs:1:{pattern}"
        ))],
        locations: Vec::new(),
        raw_input: Some(raw_input.into()),
        raw_output: Some(format!("src/lib.rs:1:{pattern}").into()),
    }
}

fn completed_find_call(pattern: &str, path: Option<&str>) -> RuntimeToolActivity {
    let mut raw_input = serde_json::json!({ "pattern": pattern });
    if let Some(path) = path {
        raw_input["path"] = serde_json::json!(path);
    }

    RuntimeToolActivity {
        activity_id: format!("call-find-{pattern}"),
        title: "Find".to_string(),
        kind: RuntimeToolKind::Search,
        status: RuntimeToolActivityStatus::Completed,
        content: vec![RuntimeToolActivityContent::Text("src/lib.rs".to_string())],
        locations: Vec::new(),
        raw_input: Some(raw_input.into()),
        raw_output: Some("src/lib.rs".into()),
    }
}

#[test]
fn runtime_writefile_in_progress_suppresses_raw_input_and_uses_compact_title() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: TEMP.md".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::InProgress,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(
                r##"{"path":"TEMP.md","content":"# TEMP\n\nraw transport content"}"##.into(),
            ),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(80, default_palette());
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(rendered_plain, vec!["● Write TEMP.md".to_string()]);
    assert!(
        lines[0].spans[0].style.fg == Some(default_palette().main),
        "active write calls should render the marker with the main text color: {:?}",
        lines[0].spans[0]
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("\"path\"") && !line.contains("\"content\"")),
        "write calls should not expose raw transport JSON in the main transcript: {rendered_plain:?}"
    );
}

#[test]
fn runtime_edit_tool_activity_header_avoids_duplicate_path_display() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit test/temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "test/temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
            }],
            locations: vec![RuntimeToolActivityLocation {
                path: "test/temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(
                serde_json::json!({
                    "path": "test/temp.md",
                    "old_string": "old\n",
                    "new_string": "new\n"
                })
                .into(),
            ),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited test/temp.md (+1 -1)");
    assert_eq!(
        rendered_plain
            .iter()
            .filter(|line| line.contains("test/temp.md"))
            .count(),
        1,
        "edit headers should show the file path only once: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().all(|line| !line.contains("Input")),
        "edit headers should not append raw input details when diff content is present: {rendered_plain:?}"
    );
}

#[test]
fn active_runtime_write_marker_blinks_by_disappearing_with_main_text_color() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: TEMP.md".to_string(),
            kind: RuntimeToolKind::Other,
            status: RuntimeToolActivityStatus::InProgress,
            content: Vec::new(),
            locations: Vec::new(),
            raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let palette = default_palette();
    let started_at = item
        .active_marker_started_at()
        .expect("active tool call should record a blink start");
    let visible = item.render_lines_at(80, palette, started_at);
    let hidden = item.render_lines_at(
        80,
        palette,
        started_at + TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
    );

    assert_eq!(line_to_plain_text(&visible[0]), "● Write TEMP.md");
    assert_eq!(line_to_plain_text(&hidden[0]), "  Write TEMP.md");
    assert_eq!(visible[0].spans[0].style.fg, Some(palette.main));
    assert!(
        !visible[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::RAPID_BLINK),
        "active marker should blink through app rendering, not terminal blink modifier"
    );
}

#[test]
fn runtime_tool_activity_diff_context_lines_keep_default_style() {
    let palette = default_palette();
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("one\nold\ntail\n".to_string()),
                new_text: "one\nnew\ntail\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    let lines = item.render_lines(80, palette);
    let context_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains(" one"))
        .expect("context line should be rendered");
    let insert_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains("+  new"))
        .expect("insert line should be rendered");
    let delete_line = lines
        .iter()
        .find(|line| line_to_plain_text(line).contains("-  old"))
        .expect("delete line should be rendered");

    assert_eq!(context_line.style.bg, None);
    assert!(
        context_line
            .spans
            .iter()
            .all(|span| span.style.bg.is_none() && span.style.fg.is_none()),
        "context diff spans should keep default styling like codex-rs: {context_line:?}"
    );
    assert!(insert_line.style.bg.is_some());
    assert!(delete_line.style.bg.is_some());
}

#[test]
fn runtime_tool_activity_added_diff_uses_codex_like_header_and_line_numbers() {
    let palette = default_palette();
    let absolute_path = std::env::current_dir()
        .expect("cwd should be available")
        .join("temp.md")
        .display()
        .to_string();
    let new_text = (1..=25)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: absolute_path,
                old_text: None,
                new_text,
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Compact,
    );
    let lines = item.render_lines(120, palette);
    let rendered_plain = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Added temp.md (+25 -0)");
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("WriteFile") && !line.contains("Diff:")),
        "diff rendering should not expose redundant tool or diff labels: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "      1 +  line 1"),
        "diff lines should right-align line numbers in a seven-column gutter: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "     25 +  line 25"),
        "compact diff should keep the tail lines: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "      ⋮ +15 lines (ctrl + t to view transcript)"),
        "compact diff omitted hint should align with the number gutter edge: {rendered_plain:?}"
    );
    assert!(
        !rendered_plain
            .iter()
            .any(|line| line.contains("13 +line 13")),
        "compact mode should omit middle diff rows: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_truncated_diff_shows_partial_preview_notice() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Write temp.md".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: true,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line.contains("preview truncated")),
        "truncated diffs should clearly say the preview is partial: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_write_kind_uses_diff_rendering() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Write temp.md".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
            }],
            locations: vec![RuntimeToolActivityLocation {
                path: "temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(
                serde_json::json!({
                    "path": "temp.md",
                    "content": "new\n"
                })
                .into(),
            ),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    "The file temp.md has been updated successfully.",
                    Some("The file temp.md has been updated successfully."),
                    Some(serde_json::json!({
                        "path": "temp.md",
                        "old_text": "old\n",
                        "new_text": "new\n"
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );

    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited temp.md (+1 -1)");
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("The file temp.md has been updated successfully")),
        "write diff rendering should prefer the diff view over the raw success payload: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_detailed_diff_keeps_all_rows() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: None,
                new_text: (1..=25)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "     13 +  line 13"),
        "detailed mode should keep middle diff rows: {rendered_plain:?}"
    );
    assert!(
        !rendered_plain
            .iter()
            .any(|line| line.contains("ctrl + t to view transcript")),
        "detailed mode should not render compact truncation hints: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_updated_diff_renders_delete_and_insert_line_numbers() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: src/lib.rs".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("one\nold\ntail\n".to_string()),
                new_text: "one\nnew\ntail\n".to_string(),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited src/lib.rs (+1 -1)");
    assert!(
        rendered_plain.iter().any(|line| line == "      2 -  old"),
        "updated diff should render old line numbers for deletions: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line == "      2 +  new"),
        "updated diff should render new line numbers for insertions: {rendered_plain:?}"
    );
    assert!(
        rendered_plain.iter().any(|line| line == "      1    one"),
        "context diff rows should right-align the line number and align content after the sign column: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("---") && !line.contains("+++")),
        "updated diff should not expose raw unified diff file headers: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_diff_suppresses_raw_input_and_output_details() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Edit test/temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "test/temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
            }],
            locations: vec![RuntimeToolActivityLocation {
                path: "test/temp.md".to_string(),
                line: None,
            }],
            raw_input: Some(serde_json::json!({
                "path": "test/temp.md",
                "old_string": "old\n",
                "new_string": "new\n"
            })
            .into()),
            raw_output: Some(
                runtime_domain::session::RuntimeToolActivityRawValue::tool_result_with_display_content(
                    "Successfully replaced 1 block(s) in test/temp.md.",
                    Some("Successfully replaced 1 block(s) in test/temp.md."),
                    Some(serde_json::json!({
                        "path": "test/temp.md",
                        "old_text": "old\n",
                        "new_text": "new\n",
                        "replacements": 1
                    })),
                ),
            ),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(rendered_plain[0], "● Edited test/temp.md (+1 -1)");
    assert!(
        rendered_plain.iter().all(|line| !line.contains("Input")),
        "diff rendering should not append raw input details: {rendered_plain:?}"
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("Successfully replaced 1 block(s)")),
        "diff rendering should not repeat the tool success payload next to the patch view: {rendered_plain:?}"
    );
    assert_eq!(
        rendered_plain
            .iter()
            .filter(|line| line.contains("test/temp.md"))
            .count(),
        1,
        "filename should appear once in the diff header: {rendered_plain:?}"
    );
}

#[test]
fn runtime_tool_activity_diff_right_aligns_three_digit_line_numbers_in_fixed_gutter() {
    let item = ToolResultItem::from_runtime_tool_activity(
        RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "WriteFile: temp.md".to_string(),
            kind: RuntimeToolKind::Edit,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Diff {
                path: "temp.md".to_string(),
                old_text: None,
                new_text: (1..=267)
                    .map(|line| format!("line {line}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
                is_truncated: false,
            }],
            locations: Vec::new(),
            raw_input: None,
            raw_output: None,
        },
        ToolActivityRenderMode::Detailed,
    );
    let rendered_plain = item
        .render_lines(120, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert!(
        rendered_plain
            .iter()
            .any(|line| line == "    267 +  line 267"),
        "three-digit line numbers should grow left within the fixed seven-column gutter: {rendered_plain:?}"
    );
}

#[test]
fn naked_shell_result_highlights_command() {
    let palette = default_palette();
    let item = ToolResultItem::new("Ran sed -n '1,80p' src/main.rs", ToolResultKind::Ran);
    let lines = item.render_lines(80, palette);

    assert_eq!(
        line_to_plain_text(&lines[0]),
        "● Ran sed -n '1,80p' src/main.rs"
    );
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
    assert_eq!(lines[0].spans[1].content.as_ref(), "Ran");
    assert!(lines[0].spans[1].style.fg.is_none());
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(2)
            .any(|span| span.style.fg.is_some()),
        "naked shell command spans should carry syntax highlight foreground colors: {:?}",
        lines[0].spans
    );
}

#[test]
fn wrapped_shell_result_uses_continuation_prefix_and_keeps_highlight() {
    let item = ToolResultItem::new(
        "Ran sed -n '1,80p' src/frontend/tui/tool_result.rs",
        ToolResultKind::Ran,
    );
    let lines = item.render_lines(18, default_palette());
    let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert!(
        plain_lines.len() > 1,
        "shell command should wrap in a narrow viewport: {plain_lines:?}"
    );
    assert!(
        plain_lines[0].starts_with("● Ran "),
        "first shell line should keep the status prefix and verb: {plain_lines:?}"
    );
    assert!(
        plain_lines[1..].iter().all(|line| line.starts_with("  ")),
        "wrapped shell continuation lines should use two leading spaces: {plain_lines:?}"
    );
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter().skip(1))
            .any(|span| span.style.fg.is_some()),
        "wrapped shell command spans should keep syntax highlight foreground colors: {lines:?}"
    );
}

#[test]
fn wrapped_result_uses_two_space_continuation_prefix() {
    let item = ToolResultItem::new("Ran Very-long-command", ToolResultKind::Ran);
    let lines = item
        .render_lines(10, default_palette())
        .into_iter()
        .map(|line| line_to_plain_text(&line))
        .collect::<Vec<_>>();

    assert_eq!(
        lines,
        vec![
            "● Very-lon".to_string(),
            "  g-comman".to_string(),
            "  d".to_string(),
        ]
    );
}

#[test]
fn terminal_default_palette_keeps_reset_style_plain() {
    let item = ToolResultItem::new("Ran echo ok", ToolResultKind::Ran);
    let line = item.render_lines(80, terminal_default_palette()).remove(0);

    assert_eq!(
        line.spans[0].style.fg,
        Some(ratatui::style::Color::LightGreen)
    );
}
