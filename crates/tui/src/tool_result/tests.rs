use ratatui::style::Modifier;

use mo_core::session::RuntimeToolKind;

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
fn acp_tool_call_header_uses_title_only_and_strips_shell_prefix() {
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

    assert_eq!(line_to_plain_text(&lines[0]), "● cargo check");
    assert_eq!(lines[0].spans[0].style.fg, Some(palette.quote));
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("Completed")),
        "status text should not be part of the ACP header: {:?}",
        lines[0].spans
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("[Other]")),
        "kind label should not be part of the ACP header: {:?}",
        lines[0].spans
    );
    assert!(
        lines[0]
            .spans
            .iter()
            .all(|span| !span.content.as_ref().contains("Shell:")),
        "tool prefix should be stripped from the ACP header: {:?}",
        lines[0].spans
    );
}

#[test]
fn acp_tool_call_header_highlights_shell_titles() {
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

    assert_eq!(line_to_plain_text(&lines[0]), "● cargo check");
    assert!(
        lines[0]
            .spans
            .iter()
            .skip(1)
            .any(|span| span.style.fg.is_some()),
        "shell-like ACP titles should carry syntax highlight foreground colors: {:?}",
        lines[0].spans
    );
}

#[test]
fn pending_execute_tool_call_renders_waiting_detail() {
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
    let rendered_plain = item
        .render_lines(80, default_palette())
        .iter()
        .map(line_to_plain_text)
        .collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec!["● cargo check".to_string(), "  └─ Waiting...".to_string()]
    );
    assert!(
        rendered_plain
            .iter()
            .all(|line| !line.contains("Requesting approval")),
        "tool call row should not duplicate the approval panel request text: {rendered_plain:?}"
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
            raw_output: Some("Checking lumos v0.1.0".into()),
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
        vec!["● cargo check".to_string(), "  └─ Waiting...".to_string()]
    );
    assert!(
        rendered_plain.iter().all(|line| {
            !line.contains("Requesting approval")
                && !line.contains("Checking lumos")
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
            raw_output: Some("Checking lumos v0.1.0".into()),
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
            "● cargo check".to_string(),
            "  └─ Checking lumos v0.1.0".to_string(),
        ]
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
            "● cargo check".to_string(),
            "  └─ Finished dev profile".to_string(),
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
            raw_output: Some("error: could not compile `lumos`".into()),
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
            "● cargo check".to_string(),
            "  └─ error: could not compile `lumos`".to_string(),
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
            "  └─ Found 3 releases".to_string(),
        ]
    );
}

#[test]
fn acp_tool_call_raw_output_trailing_newline_does_not_render_blank_line() {
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
            raw_output: Some("Checking lumos\n".into()),
        },
        ToolActivityRenderMode::Compact,
    );
    let rendered = item.render_lines(80, palette);
    let rendered_plain = rendered.iter().map(line_to_plain_text).collect::<Vec<_>>();

    assert_eq!(
        rendered_plain,
        vec![
            "● cargo check".to_string(),
            "  └─ Checking lumos".to_string(),
        ]
    );
    assert!(
        rendered
            .last()
            .is_some_and(|line| !line_to_plain_text(line).trim().is_empty()),
        "rendered ACP output should not end with a blank line: {rendered_plain:?}"
    );
}

#[test]
fn acp_pending_text_content_is_not_approval_waiting_without_permission_state() {
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
fn acp_tool_call_multi_line_raw_output_uses_four_space_continuation_prefix() {
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
            "● git log --oneline -5".to_string(),
            "  └─ first line".to_string(),
            "    second line".to_string(),
        ]
    );
}

#[test]
fn acp_tool_call_terminal_content_renders_live_snapshot() {
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
        mo_core::session::RuntimeTerminalSnapshot {
            terminal_id: "term-1".to_string(),
            command: Some("cargo check".to_string()),
            cwd: None,
            output: "Checking lumos\nFinished".to_string(),
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
    assert!(plain.contains("Checking lumos"));
    assert!(plain.contains("Finished"));
    assert!(!plain.contains("ACP terminal unavailable"));
    assert!(!plain.contains("terminal/create unsupported"));
}

#[test]
fn acp_tool_call_raw_output_uses_secondary_color_and_codex_like_alignment() {
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
                    "Checking lumos v0.1.0 (/home/archie/GoCodes/lumos_rust)\nFinished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s"
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
            "● cargo check".to_string(),
            "  └─ Checking lumos v0.1.0 (/home/archie/GoCodes/lumos_rust)".to_string(),
            "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.01s".to_string(),
        ]
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
fn acp_read_tool_call_renders_compact_summary_without_content_details() {
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
fn acp_readfile_title_fallback_renders_compact_summary_even_without_read_kind() {
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
        "acp.example.toml",
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
                "    acp.example.toml".to_string(),
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
            "● Read AGENTS.md".to_string(),
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

#[test]
fn acp_writefile_in_progress_suppresses_raw_input_and_uses_compact_title() {
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
fn active_acp_write_marker_blinks_by_disappearing_with_main_text_color() {
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
fn acp_tool_call_diff_context_lines_keep_default_style() {
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
fn acp_tool_call_added_diff_uses_codex_like_header_and_line_numbers() {
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
fn acp_tool_call_detailed_diff_keeps_all_rows() {
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
fn acp_tool_call_updated_diff_renders_delete_and_insert_line_numbers() {
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
fn acp_tool_call_diff_right_aligns_three_digit_line_numbers_in_fixed_gutter() {
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
