use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mo_tools::{ToolCall, ToolExecutor, builtin::workspace_tool_registry};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn builtin_edit_rejects_partial_read_snapshot() {
    let root = temp_root("mutation-edit-partial-read");
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
    assert!(!read_result.is_error);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "old_string": "two",
                    "new_string": "changed"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.contains("has not been read"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        "one\ntwo\nthree\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_missing_file_when_old_string_is_non_empty() {
    let root = temp_root("mutation-edit-missing-old-string");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "missing.txt",
                    "old_string": "old",
                    "new_string": "new"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert!(result.content.contains("File does not exist"));
    assert!(!root.join("missing.txt").exists());
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_and_edit_reject_directory_paths() {
    let root = temp_root("mutation-directory-paths");
    fs::create_dir(root.join("src")).expect("create fixture directory");
    let registry = workspace_tool_registry(&root);

    let write_result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "src",
                    "content": "not a directory anymore"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(write_result.is_error);
    assert!(write_result.content.contains("is a directory"));

    let edit_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "src",
                    "old_string": "",
                    "new_string": "created"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(edit_result.is_error);
    assert!(edit_result.content.contains("is a directory"));

    assert!(root.join("src").is_dir());
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_reports_noop_and_missing_match_without_modifying_file() {
    let root = temp_root("mutation-edit-noop-missing");
    fs::write(root.join("notes.txt"), "alpha\nbeta\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error);

    let noop_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "old_string": "alpha",
                    "new_string": "alpha"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(noop_result.is_error);
    assert!(noop_result.content.contains("No changes to make"));

    let missing_match_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-2",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "old_string": "missing",
                    "new_string": "replacement"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(missing_match_result.is_error);
    assert!(missing_match_result.content.contains("not found"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        "alpha\nbeta\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_preserves_utf8_bom_and_crlf_line_endings() {
    let root = temp_root("mutation-edit-bom-crlf");
    fs::write(root.join("notes.txt"), "\u{feff}alpha\r\nbeta\r\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "old_string": "beta\n",
                    "new_string": "gamma\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error, "edit should succeed: {result:?}");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        "\u{feff}alpha\r\ngamma\r\n"
    );
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
