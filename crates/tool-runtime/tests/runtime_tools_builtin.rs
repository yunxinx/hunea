use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ToolCall, ToolExecutionContext, ToolExecutor, ToolExecutorRegistry, ToolKind,
    ToolPermissionPolicy, ToolProgress, ToolProgressSink, ToolResultContent,
    builtin::{
        bash_tool, find_tool, grep_tool, list_dir_tool, read_tool, view_image_tool,
        workspace_readonly_tool_registry, workspace_tool_registry,
    },
};

#[test]
fn builtin_workspace_readonly_registry_exposes_file_tools() {
    let root = temp_root("builtin-definitions");
    let registry = workspace_readonly_tool_registry(&root);
    let definitions = registry.definitions();

    assert!(definitions.definition("read").is_some());
    assert!(definitions.definition("view_image").is_some());
    assert!(definitions.definition("list_dir").is_some());
    assert!(definitions.definition("grep").is_some());
    assert!(definitions.definition("find").is_some());
    assert_eq!(
        definitions
            .definition("read")
            .map(|definition| definition.kind),
        Some(ToolKind::Read)
    );
    assert_eq!(
        definitions
            .definition("view_image")
            .map(|definition| definition.kind),
        Some(ToolKind::Read)
    );
    assert_eq!(
        definitions
            .definition("list_dir")
            .map(|definition| definition.kind),
        Some(ToolKind::Search)
    );
    assert_eq!(
        definitions
            .definition("grep")
            .map(|definition| definition.kind),
        Some(ToolKind::Search)
    );
    assert_eq!(
        definitions
            .definition("find")
            .map(|definition| definition.kind),
        Some(ToolKind::Search)
    );

    cleanup(&root);
}

#[test]
fn builtin_workspace_registry_exposes_read_write_and_edit_tools() {
    let root = temp_root("builtin-writable-definitions");
    let registry = workspace_tool_registry(&root);
    let definitions = registry.definitions();

    assert!(definitions.definition("read").is_some());
    assert!(definitions.definition("view_image").is_some());
    assert!(definitions.definition("list_dir").is_some());
    assert!(definitions.definition("grep").is_some());
    assert!(definitions.definition("find").is_some());
    assert_eq!(
        definitions
            .definition("write")
            .map(|definition| definition.kind),
        Some(ToolKind::Write)
    );
    assert_eq!(
        definitions
            .definition("edit")
            .map(|definition| definition.kind),
        Some(ToolKind::Edit)
    );
    assert_eq!(
        definitions
            .definition("bash")
            .map(|definition| definition.kind),
        Some(ToolKind::Execute)
    );

    cleanup(&root);
}

#[test]
fn builtin_readonly_registry_does_not_expose_write_tools() {
    let root = temp_root("builtin-readonly-no-write");
    let registry = workspace_readonly_tool_registry(&root);
    let definitions = registry.definitions();

    assert!(definitions.definition("write").is_none());
    assert!(definitions.definition("edit").is_none());

    cleanup(&root);
}

#[test]
fn builtin_readonly_file_tools_are_approved_by_default() {
    let root = temp_root("builtin-permissions");
    let registry = workspace_readonly_tool_registry(&root);
    let definitions = registry.definitions();

    assert_eq!(
        definitions
            .definition("read")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Always)
    );
    assert_eq!(
        definitions
            .definition("list_dir")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Always)
    );
    assert_eq!(
        definitions
            .definition("grep")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Always)
    );
    assert_eq!(
        definitions
            .definition("find")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Always)
    );

    cleanup(&root);
}

#[test]
fn builtin_write_tools_require_ask_permission_by_default() {
    let root = temp_root("builtin-write-permissions");
    let registry = workspace_tool_registry(&root);
    let definitions = registry.definitions();

    assert_eq!(
        definitions
            .definition("write")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Ask)
    );
    assert_eq!(
        definitions
            .definition("edit")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Ask)
    );
    assert_eq!(
        definitions
            .definition("bash")
            .map(|definition| definition.permission_policy),
        Some(ToolPermissionPolicy::Ask)
    );

    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_tool_can_be_registered_independently() {
    let root = temp_root("builtin-bash");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));
    let definitions = registry.definitions();

    let definition = definitions
        .definition("bash")
        .expect("bash definition should be registered");
    assert_eq!(definition.kind, ToolKind::Execute);
    assert_eq!(definition.permission_policy, ToolPermissionPolicy::Ask);
    assert!(
        definition
            .input_schema
            .as_ref()
            .and_then(|schema| schema.get("properties"))
            .and_then(|properties| properties.get("description"))
            .is_some(),
        "bash schema should expose an optional description field"
    );
    assert!(definitions.definition("read").is_none());

    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_rejects_legacy_reason_argument() {
    let root = temp_root("builtin-bash-legacy-reason");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'hi\\n'",
                    "reason": "Legacy argument should not be accepted"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("reason"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_returns_merged_stdout_and_stderr() {
    let root = temp_root("builtin-bash-merged-output");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'stdout\\n'; printf 'stderr\\n' >&2"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "bash command should succeed: {result:?}"
    );
    assert!(result.content().contains("stdout"));
    assert!(result.content().contains("stderr"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_records_duration_metadata() {
    let root = temp_root("builtin-bash-duration");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'done\\n'"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "bash command should succeed: {result:?}"
    );
    assert!(
        result
            .details()
            .and_then(|details| details.get("duration_ms"))
            .and_then(serde_json::Value::as_u64)
            .is_some(),
        "bash result details should include duration_ms: {:?}",
        result.details()
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_non_zero_exit_returns_error_with_output() {
    let root = temp_root("builtin-bash-non-zero");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'before failure\\n'; exit 7"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("before failure"));
    assert!(result.content().contains("Command exited with code 7"));
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("exit_code"))
            .and_then(serde_json::Value::as_i64),
        Some(7)
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_timeout_kills_command_and_keeps_output() {
    let root = temp_root("builtin-bash-timeout");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'started\\n'; sleep 2; printf 'after sleep\\n'",
                    "timeout": 0.1
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("started"));
    assert!(!result.content().contains("after sleep"));
    assert!(
        result
            .content()
            .contains("Command timed out after 0.1 seconds")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("timed_out"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_cancellation_kills_command_and_keeps_output() {
    let root = temp_root("builtin-bash-cancel");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));
    let cancellation = CancellationToken::new();
    let cancellation_task = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        cancellation_task.cancel();
    });

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'started\\n'; sleep 2; printf 'after sleep\\n'"
                }),
            ),
            &cancellation,
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("started"));
    assert!(!result.content().contains("after sleep"));
    assert!(result.content().contains("Command aborted"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_runs_inside_workspace_workdir() {
    let root = temp_root("builtin-bash-workdir");
    fs::create_dir(root.join("nested")).expect("create nested dir");
    fs::write(root.join("nested/marker.txt"), "marker\n").expect("write fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "pwd; ls marker.txt",
                    "workdir": "nested"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "bash workdir should succeed: {result:?}"
    );
    assert!(result.content().contains("nested"));
    assert!(result.content().contains("marker.txt"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_rejects_workdir_outside_workspace() {
    let root = temp_root("builtin-bash-workdir-root");
    let outside = temp_root("builtin-bash-workdir-outside");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "pwd",
                    "workdir": outside
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("outside workspace"));
    cleanup(&outside);
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_truncates_large_output_and_persists_full_output_path() {
    let root = temp_root("builtin-bash-truncate");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "i=1; while [ \"$i\" -le 2105 ]; do echo \"$i\"; i=$((i+1)); done"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "large output should succeed: {result:?}"
    );
    assert!(result.content().contains("2105"));
    let expected_display_content = (106..=2105)
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        result.display_content(),
        Some(expected_display_content.as_str()),
        "TUI display output should not include the model-visible full-output footer"
    );
    assert!(!result.content().contains("\n1\n"));
    let details = result.details().expect("details should exist");
    assert_eq!(
        details
            .get("truncated")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let full_output_path = details
        .get("full_output_path")
        .and_then(serde_json::Value::as_str)
        .expect("full output path should be recorded");
    let full_output = fs::read_to_string(full_output_path).expect("read full output");
    assert!(full_output.starts_with("1\n2\n"));
    assert!(full_output.contains("2105\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::metadata(full_output_path)
            .expect("stat full output")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(permissions, 0o600);
    }
    fs::remove_file(full_output_path).expect("remove full output fixture");
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_byte_truncation_uses_accurate_model_footer() {
    let root = temp_root("builtin-bash-byte-truncate");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf '%*s' 60000 '' | tr ' ' x"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "large one-line output should succeed: {result:?}"
    );
    assert!(
        result.content().contains("[Showing last "),
        "byte truncation should describe a byte tail, not a complete line range: {}",
        result.content()
    );
    assert!(
        !result.content().contains("[Showing lines "),
        "byte truncation should not claim complete line ranges: {}",
        result.content()
    );
    assert!(
        result
            .display_content()
            .is_some_and(|content| !content.contains("[Showing last ")),
        "display content should stay free of model metadata: {:?}",
        result.display_content()
    );

    if let Some(full_output_path) = result
        .details()
        .and_then(|details| details.get("full_output_path"))
        .and_then(serde_json::Value::as_str)
    {
        let _ = fs::remove_file(full_output_path);
    }
    cleanup(&root);
}

#[tokio::test]
async fn builtin_bash_emits_terminal_progress_snapshots() {
    let root = temp_root("builtin-bash-terminal-progress");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(bash_tool(&root));
    let cancellation = CancellationToken::new();
    let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::unbounded_channel();

    let result = registry
        .execute_tool_with_context(
            ToolCall::new(
                "call-1",
                "bash",
                serde_json::json!({
                    "command": "printf 'progress\\n'"
                }),
            ),
            ToolExecutionContext::new(&cancellation)
                .with_progress_sink(ToolProgressSink::from_sender(progress_sender)),
        )
        .await;

    assert!(!result.is_error());
    let mut snapshots = Vec::new();
    while let Ok(progress) = progress_receiver.try_recv() {
        match progress {
            ToolProgress::SystemMessage { .. } => {}
            ToolProgress::TerminalUpdated { snapshot } => snapshots.push(snapshot),
            ToolProgress::ManagedSearchToolAuthorization { .. } => {}
        }
    }
    assert!(
        snapshots.iter().any(|snapshot| {
            snapshot.terminal_id == "call-1"
                && snapshot.command.as_deref() == Some("printf 'progress\\n'")
        }),
        "bash should emit an initial terminal snapshot: {snapshots:?}"
    );
    assert!(
        snapshots.iter().any(|snapshot| {
            snapshot.terminal_id == "call-1"
                && snapshot.output.contains("progress")
                && snapshot.exit_status.is_some()
                && snapshot.released
        }),
        "bash should emit a final terminal snapshot: {snapshots:?}"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_tool_can_be_registered_independently() {
    let root = temp_root("builtin-read");
    fs::write(root.join("notes.txt"), "one\ntwo\nthree\n").expect("write fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(read_tool(&root));
    let definitions = registry.definitions();

    assert!(definitions.definition("read").is_some());
    assert!(definitions.definition("list_dir").is_none());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "read",
                serde_json::json!({
                    "path": "notes.txt",
                    "offset": 2,
                    "limit": 2
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert_eq!(result.text_content(), "2\ttwo\n3\tthree");
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_tool_can_be_registered_independently() {
    let root = temp_root("builtin-view-image");
    fs::write(root.join("pixel.png"), png_fixture()).expect("write image fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(view_image_tool(&root));
    let definitions = registry.definitions();

    assert!(definitions.definition("view_image").is_some());
    assert!(definitions.definition("read").is_none());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "pixel.png" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert!(matches!(
        result.content().as_slice(),
        [ToolResultContent::Image { data_base64, mime_type, uri, detail }]
            if !data_base64.is_empty()
                && mime_type == "image/png"
                && uri.as_deref() == Some("pixel.png")
                && detail == &Some(tool_runtime::ToolImageDetail::High)
    ));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_tool_can_be_registered_independently() {
    let root = temp_root("builtin-list-dir");
    fs::create_dir(root.join("src")).expect("create src dir");
    fs::write(root.join("Cargo.toml"), "[package]\n").expect("write fixture");
    fs::write(root.join(".hidden"), "hidden\n").expect("write hidden fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(list_dir_tool(&root));
    let definitions = registry.definitions();

    assert!(definitions.definition("list_dir").is_some());
    assert!(definitions.definition("read").is_none());
    let list_dir_schema_properties = definitions
        .definition("list_dir")
        .and_then(|definition| definition.input_schema.as_ref())
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
        .expect("list_dir schema should expose object properties");
    assert_eq!(list_dir_schema_properties.len(), 2);
    assert!(list_dir_schema_properties.contains_key("path"));
    assert!(list_dir_schema_properties.contains_key("limit"));

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "list_dir", serde_json::json!({})),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert!(result.content().contains("Cargo.toml"));
    assert!(result.content().contains("src/"));
    assert!(result.content().contains(".hidden"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_grep_tool_can_be_registered_independently() {
    let root = temp_root("builtin-grep");
    fs::create_dir(root.join("src")).expect("create src dir");
    fs::write(
        root.join("src").join("lib.rs"),
        "fn keep() {\n    needle();\n}\n",
    )
    .expect("write fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(grep_tool(&root));
    let definitions = registry.definitions();

    let definition = definitions
        .definition("grep")
        .expect("grep definition should be registered");
    assert_eq!(definition.kind, ToolKind::Search);
    assert_eq!(definition.permission_policy, ToolPermissionPolicy::Always);
    assert!(definitions.definition("find").is_none());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "grep",
                serde_json::json!({
                    "pattern": "needle",
                    "path": ".",
                    "glob": "*.rs"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "grep should succeed: {result:?}");
    assert!(result.content().contains("src/lib.rs:2:"));
    assert!(result.content().contains("needle();"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_grep_searches_hidden_files_and_truncates_long_lines_by_default() {
    let root = temp_root("builtin-grep-hidden-long-line");
    let long_match = format!("needle {}", "x".repeat(700));
    fs::write(root.join(".hidden.rs"), format!("{long_match}\n")).expect("write hidden fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(grep_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "grep",
                serde_json::json!({
                    "pattern": "needle",
                    "literal": true
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "grep should succeed: {result:?}");
    assert!(result.content().contains(".hidden.rs:1:"));
    assert!(result.content().contains("needle "));
    assert!(
        result.content().contains("..."),
        "long matching line should be visibly truncated: {}",
        result.content()
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("truncated"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("lines_truncated"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_grep_applies_byte_truncation_after_match_limit() {
    let root = temp_root("builtin-grep-byte-limit");
    let mut content = String::new();
    for index in 0..200 {
        content.push_str(&format!("needle-{index:03} {}\n", "x".repeat(350)));
    }
    fs::write(root.join("large.rs"), content).expect("write large fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(grep_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "grep",
                serde_json::json!({
                    "pattern": "needle",
                    "literal": true,
                    "limit": 200
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "grep should succeed: {result:?}");
    assert!(result.content().contains("50.0KB limit reached"));
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("truncated"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("byte_truncated"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_find_tool_can_be_registered_independently() {
    let root = temp_root("builtin-find");
    fs::create_dir_all(root.join("src").join("bin")).expect("create nested src dir");
    fs::write(root.join(".gitignore"), "target/\n").expect("write gitignore");
    fs::write(root.join("src").join("lib.rs"), "pub fn lib() {}\n").expect("write lib fixture");
    fs::write(
        root.join("src").join("bin").join("main.rs"),
        "fn main() {}\n",
    )
    .expect("write main fixture");
    fs::create_dir(root.join("target")).expect("create target dir");
    fs::write(root.join("target").join("ignored.rs"), "ignored\n").expect("write ignored fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(find_tool(&root));
    let definitions = registry.definitions();

    let definition = definitions
        .definition("find")
        .expect("find definition should be registered");
    assert_eq!(definition.kind, ToolKind::Search);
    assert_eq!(definition.permission_policy, ToolPermissionPolicy::Always);
    assert!(definitions.definition("grep").is_none());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "find",
                serde_json::json!({
                    "pattern": "*.rs",
                    "path": "."
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "find should succeed: {result:?}");
    assert_eq!(result.text_content(), "src/bin/main.rs\nsrc/lib.rs");
    cleanup(&root);
}

#[tokio::test]
async fn builtin_find_uses_required_glob_pattern_and_searches_hidden_paths_by_default() {
    let root = temp_root("builtin-find-pi-like-glob");
    fs::create_dir_all(root.join("src").join("bin")).expect("create nested src dir");
    fs::write(root.join("src").join("lib.rs"), "pub fn lib() {}\n").expect("write lib fixture");
    fs::write(
        root.join("src").join("bin").join("main.rs"),
        "fn main() {}\n",
    )
    .expect("write main fixture");
    fs::write(root.join(".hidden.rs"), "hidden\n").expect("write hidden fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(find_tool(&root));
    let definitions = registry.definitions();
    let schema = definitions
        .definition("find")
        .and_then(|definition| definition.input_schema.as_ref())
        .expect("find schema should exist");
    let properties = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .expect("find schema should expose properties");

    assert_eq!(properties.len(), 3);
    assert!(properties.contains_key("pattern"));
    assert!(properties.contains_key("path"));
    assert!(properties.contains_key("limit"));
    assert_eq!(
        schema.get("required"),
        Some(&serde_json::json!(["pattern"]))
    );

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "find",
                serde_json::json!({
                    "pattern": "*.rs"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "find should succeed: {result:?}");
    assert!(result.content().contains(".hidden.rs"));
    assert!(result.content().contains("src/lib.rs"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_find_treats_path_containing_pattern_as_full_path_glob() {
    let root = temp_root("builtin-find-full-path-glob");
    fs::create_dir_all(root.join("src").join("bin")).expect("create nested src dir");
    fs::write(root.join("src").join("lib.rs"), "pub fn lib() {}\n").expect("write lib fixture");
    fs::write(
        root.join("src").join("bin").join("main.rs"),
        "fn main() {}\n",
    )
    .expect("write main fixture");
    fs::write(root.join("main.rs"), "fn root_main() {}\n").expect("write root fixture");
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(find_tool(&root));

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "find",
                serde_json::json!({
                    "pattern": "src/**/*.rs"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "find should succeed: {result:?}");
    assert!(result.content().contains("src/bin/main.rs"));
    assert!(result.content().contains("src/lib.rs"));
    assert!(!result.content().contains("\nmain.rs"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_search_tools_exclude_vcs_directories_but_keep_hidden_files() {
    let root = temp_root("builtin-search-vcs-hidden");
    fs::create_dir_all(root.join(".git")).expect("create git dir");
    fs::write(root.join(".git").join("config"), "needle vcs\n").expect("write git fixture");
    fs::write(root.join(".hidden.rs"), "needle hidden\n").expect("write hidden fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let grep_result = registry
        .execute_tool(
            ToolCall::new(
                "grep-1",
                "grep",
                serde_json::json!({
                    "pattern": "needle",
                    "literal": true,
                    "limit": 10
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !grep_result.is_error(),
        "grep should succeed: {grep_result:?}"
    );
    assert!(grep_result.content().contains(".hidden.rs"));
    assert!(!grep_result.content().contains(".git/config"));

    let find_result = registry
        .execute_tool(
            ToolCall::new(
                "find-1",
                "find",
                serde_json::json!({
                    "pattern": "*",
                    "limit": 20
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !find_result.is_error(),
        "find should succeed: {find_result:?}"
    );
    assert!(find_result.content().contains(".hidden.rs"));
    assert!(!find_result.content().contains(".git"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_grep_and_find_reject_arguments_outside_schema_before_execution() {
    let root = temp_root("builtin-search-schema-extra");
    fs::write(root.join("notes.txt"), "needle\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let grep_result = registry
        .execute_tool(
            ToolCall::new(
                "grep-1",
                "grep",
                serde_json::json!({
                    "pattern": "needle",
                    "recursive": false
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(grep_result.is_error());
    assert!(
        grep_result
            .content()
            .contains("arguments do not match schema")
    );
    assert!(grep_result.content().contains("recursive"));

    let find_result = registry
        .execute_tool(
            ToolCall::new(
                "find-1",
                "find",
                serde_json::json!({
                    "pattern": "*.txt",
                    "entry_type": "file"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(find_result.is_error());
    assert!(
        find_result
            .content()
            .contains("arguments do not match schema")
    );
    assert!(find_result.content().contains("entry_type"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_creates_missing_file_without_prior_read() {
    let root = temp_root("builtin-write-create");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "write",
                serde_json::json!({
                    "path": "nested/notes.txt",
                    "content": "one\ntwo\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "write should create missing files: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("nested/notes.txt")).expect("read created file"),
        "one\ntwo\n"
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("path"))
            .and_then(serde_json::Value::as_str),
        Some("nested/notes.txt")
    );
    assert_eq!(
        result.details().and_then(|details| details.get("old_text")),
        None
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("new_text"))
            .and_then(serde_json::Value::as_str),
        Some("one\ntwo\n")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_rejects_missing_paths_outside_workspace() {
    let root = temp_root("builtin-write-outside-root");
    let outside = temp_root("builtin-write-outside-target");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "write",
                serde_json::json!({
                    "path": outside.join("created.txt"),
                    "content": "secret\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("outside workspace"));
    assert!(!outside.join("created.txt").exists());
    cleanup(&outside);
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_existing_file_requires_complete_prior_read() {
    let root = temp_root("builtin-write-requires-read");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "new\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("has not been read"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        "old\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_rejects_partial_read_snapshot() {
    let root = temp_root("builtin-write-partial-read");
    fs::write(root.join("notes.txt"), "one\ntwo\nthree\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new(
                "read-1",
                "read",
                serde_json::json!({
                    "path": "notes.txt",
                    "offset": 1,
                    "limit": 2
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "replacement\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("has not been read"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_rejects_read_snapshot_with_truncated_line() {
    let root = temp_root("builtin-write-truncated-line-read");
    let original = format!("{}\n", "a".repeat(2_100));
    fs::write(root.join("notes.txt"), &original).expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error());
    assert!(
        read_result.content().ends_with("..."),
        "long line should be visibly truncated: {}",
        read_result.content()
    );

    let result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "replacement\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("has not been read"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        original
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_existing_file_succeeds_after_complete_read() {
    let root = temp_root("builtin-write-after-read");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error());

    let result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "new\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "write after read should succeed: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read updated fixture"),
        "new\n"
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("old_text"))
            .and_then(serde_json::Value::as_str),
        Some("old\n")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("new_text"))
            .and_then(serde_json::Value::as_str),
        Some("new\n")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_rejects_file_changed_after_read() {
    let root = temp_root("builtin-write-stale");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error());
    fs::write(root.join("notes.txt"), "external\n").expect("modify fixture outside tool");

    let result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "new\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("modified since read"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read stale fixture"),
        "external\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_omits_gitignored_entries_by_default() {
    let root = temp_root("builtin-list-dir-gitignore");
    fs::write(root.join(".gitignore"), "target/\n*.tmp\n").expect("write gitignore");
    fs::create_dir(root.join("src")).expect("create src dir");
    fs::create_dir(root.join("target")).expect("create ignored target dir");
    fs::write(root.join(".hidden"), "hidden\n").expect("write hidden fixture");
    fs::write(root.join("scratch.tmp"), "ignored\n").expect("write ignored tmp fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "list_dir", serde_json::json!({ "path": "." })),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert!(result.content().contains(".hidden"));
    assert!(result.content().contains("src/"));
    assert!(!result.content().contains("target/"));
    assert!(!result.content().contains("scratch.tmp"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_rejects_arguments_outside_schema_before_execution() {
    let root = temp_root("builtin-list-dir-schema-extra");
    fs::write(root.join("Cargo.toml"), "[package]\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "list_dir",
                serde_json::json!({
                    "path": ".",
                    "recursive": true
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("recursive"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_rejects_offset_below_schema_minimum() {
    let root = temp_root("builtin-read-schema-minimum");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "read",
                serde_json::json!({
                    "path": "notes.txt",
                    "offset": 0
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("offset"));
    assert!(result.content().contains("minimum"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_limits_output_entries() {
    let root = temp_root("builtin-list-dir-limit");
    fs::write(root.join("a.txt"), "a\n").expect("write fixture");
    fs::write(root.join("b.txt"), "b\n").expect("write fixture");
    fs::write(root.join("c.txt"), "c\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "list_dir",
                serde_json::json!({
                    "path": ".",
                    "limit": 2
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert!(result.content().contains("a.txt"));
    assert!(result.content().contains("b.txt"));
    assert!(!result.content().contains("c.txt"));
    assert!(result.content().contains("Truncated"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_rejects_paths_outside_workspace_root() {
    let root = temp_root("builtin-outside-root");
    let outside = temp_root("builtin-outside-target");
    fs::write(outside.join("secret.txt"), "secret\n").expect("write outside fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "read",
                serde_json::json!({ "path": outside.join("secret.txt") }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("outside workspace"));
    cleanup(&outside);
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_directs_image_files_to_view_image() {
    let root = temp_root("builtin-read-image");
    fs::write(
        root.join("pixel.png"),
        [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H',
            b'D', b'R',
        ],
    )
    .expect("write image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "read", serde_json::json!({ "path": "pixel.png" })),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result
            .text_content()
            .contains("image/png files are image content")
    );
    assert!(
        result
            .text_content()
            .contains("use view_image instead of read")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_directory_paths() {
    let root = temp_root("builtin-view-image-directory");
    fs::create_dir(root.join("assets")).expect("create directory fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "assets" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.text_content().contains("is not a file"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_paths_outside_workspace_root() {
    let root = temp_root("builtin-view-image-outside-root");
    let outside = temp_root("builtin-view-image-outside-target");
    fs::write(outside.join("pixel.png"), png_fixture()).expect("write outside image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": outside.join("pixel.png") }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.text_content().contains("outside workspace"));
    cleanup(&outside);
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_unsupported_detail_values() {
    let root = temp_root("builtin-view-image-detail");
    fs::write(root.join("pixel.png"), png_fixture()).expect("write image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "pixel.png", "detail": "low" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result
            .text_content()
            .contains("view_image arguments are invalid")
    );
    assert!(result.text_content().contains("unknown variant `low`"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_preserves_original_detail_hint() {
    let root = temp_root("builtin-view-image-original-detail");
    fs::write(root.join("pixel.png"), png_fixture()).expect("write image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "pixel.png", "detail": "original" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error());
    assert!(matches!(
        result.content().as_slice(),
        [ToolResultContent::Image { detail, .. }]
            if detail == &Some(tool_runtime::ToolImageDetail::Original)
    ));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_unsupported_extensions() {
    let root = temp_root("builtin-view-image-extension");
    fs::write(root.join("image.bmp"), [b'B', b'M', 0x00, 0x00]).expect("write bmp fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "image.bmp" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.text_content().contains("unsupported image type"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_oversized_images_before_encoding() {
    let root = temp_root("builtin-view-image-oversized");
    let image_path = root.join("large.png");
    fs::write(&image_path, png_fixture()).expect("write image fixture");
    fs::OpenOptions::new()
        .write(true)
        .open(&image_path)
        .expect("open image fixture")
        .set_len(20 * 1024 * 1024 + 1)
        .expect("extend image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "large.png" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.text_content().contains("too large for view_image"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_view_image_rejects_invalid_image_signatures() {
    let root = temp_root("builtin-view-image-invalid");
    fs::write(root.join("pixel.png"), b"not really a png").expect("write invalid image fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "view_image",
                serde_json::json!({ "path": "pixel.png" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result
            .text_content()
            .contains("does not look like image/png")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_rejects_binary_text_fallback() {
    let root = temp_root("builtin-read-binary");
    fs::write(
        root.join("blob.dat"),
        [0x66, 0x6f, 0x6f, 0x00, 0x62, 0x61, 0x72],
    )
    .expect("write binary fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "read", serde_json::json!({ "path": "blob.dat" })),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("not valid UTF-8 text"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_rejects_control_character_binary_payload() {
    let root = temp_root("builtin-read-control-bytes");
    fs::write(root.join("archive.zip"), [0x50, 0x4b, 0x03, 0x04]).expect("write zip fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "call-1",
                "read",
                serde_json::json!({ "path": "archive.zip" }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("not valid UTF-8 text"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_returns_interrupted_when_cancellation_is_pre_triggered() {
    let root = temp_root("builtin-read-cancelled");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &cancellation,
        )
        .await;

    assert!(result.is_error());
    assert_eq!(result.text_content(), "Tool call interrupted");
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_returns_interrupted_when_cancellation_is_pre_triggered() {
    let root = temp_root("builtin-list-dir-cancelled");
    fs::create_dir(root.join("src")).expect("create src dir");
    let registry = workspace_readonly_tool_registry(&root);
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "list_dir", serde_json::json!({ "path": "." })),
            &cancellation,
        )
        .await;

    assert!(result.is_error());
    assert_eq!(result.text_content(), "Tool call interrupted");
    cleanup(&root);
}

fn temp_root(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn png_fixture() -> Vec<u8> {
    vec![
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D',
        b'R',
    ]
}
