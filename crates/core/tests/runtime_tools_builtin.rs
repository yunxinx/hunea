use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mo_core::tools::{
    RuntimeToolCall, RuntimeToolExecutor, RuntimeToolResult,
    builtin::workspace_readonly_tool_registry,
};
use tokio_util::sync::CancellationToken;

#[test]
fn builtin_workspace_readonly_registry_exposes_file_tools() {
    let root = temp_root("builtin-definitions");
    let registry = workspace_readonly_tool_registry(&root);
    let definitions = registry.definitions();

    assert!(definitions.definition("file_read").is_some());
    assert!(definitions.definition("list_dir").is_some());

    cleanup(&root);
}

#[tokio::test]
async fn builtin_file_read_reads_requested_line_range() {
    let root = temp_root("builtin-file-read");
    fs::write(root.join("notes.txt"), "one\ntwo\nthree\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            RuntimeToolCall::new(
                "call-1",
                "file_read",
                serde_json::json!({
                    "path": "notes.txt",
                    "start_line": 2,
                    "end_line": 3
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert_eq!(
        result,
        RuntimeToolResult::success("call-1", "2\ttwo\n3\tthree")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_lists_workspace_relative_entries() {
    let root = temp_root("builtin-list-dir");
    fs::create_dir(root.join("src")).expect("create src dir");
    fs::write(root.join("Cargo.toml"), "[package]\n").expect("write fixture");
    fs::write(root.join(".hidden"), "hidden\n").expect("write hidden fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            RuntimeToolCall::new("call-1", "list_dir", serde_json::json!({ "path": "." })),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error);
    assert!(result.content.contains("Cargo.toml"));
    assert!(result.content.contains("src/"));
    assert!(!result.content.contains(".hidden"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_list_dir_rejects_arguments_outside_schema_before_execution() {
    let root = temp_root("builtin-list-dir-schema-extra");
    fs::write(root.join("Cargo.toml"), "[package]\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            RuntimeToolCall::new(
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

    assert!(result.is_error);
    assert!(result.content.contains("arguments do not match schema"));
    assert!(result.content.contains("recursive"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_file_read_rejects_line_number_below_schema_minimum() {
    let root = temp_root("builtin-file-read-schema-minimum");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            RuntimeToolCall::new(
                "call-1",
                "file_read",
                serde_json::json!({
                    "path": "notes.txt",
                    "start_line": 0
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.contains("arguments do not match schema"));
    assert!(result.content.contains("start_line"));
    assert!(result.content.contains("minimum"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_file_read_rejects_paths_outside_workspace_root() {
    let root = temp_root("builtin-outside-root");
    let outside = temp_root("builtin-outside-target");
    fs::write(outside.join("secret.txt"), "secret\n").expect("write outside fixture");
    let registry = workspace_readonly_tool_registry(&root);

    let result = registry
        .execute_tool(
            RuntimeToolCall::new(
                "call-1",
                "file_read",
                serde_json::json!({ "path": outside.join("secret.txt") }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.contains("outside workspace"));
    cleanup(&outside);
    cleanup(&root);
}

fn temp_root(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("lumos-{prefix}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}
