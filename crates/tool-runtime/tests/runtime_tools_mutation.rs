use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ToolCall, ToolExecutionContext, ToolExecutor, builtin::workspace_tool_registry,
};

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
                    "edits": [
                        { "old_string": "two", "new_string": "changed" }
                    ]
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
async fn builtin_edit_rejects_missing_files() {
    let root = temp_root("mutation-edit-missing-file");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "missing.txt",
                    "edits": [
                        { "old_string": "old", "new_string": "new" }
                    ]
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
                    "edits": [
                        { "old_string": "old", "new_string": "new" }
                    ]
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
                    "edits": [
                        { "old_string": "alpha", "new_string": "alpha" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(noop_result.is_error);
    assert!(noop_result.content.contains("No changes"));

    let missing_match_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-2",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "missing", "new_string": "replacement" }
                    ]
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
async fn builtin_write_can_follow_successful_edit_without_another_read() {
    let root = temp_root("mutation-write-after-edit");
    fs::write(root.join("notes.txt"), "one\ntwo\nthree\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error);

    let edit_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "two\n", "new_string": "" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !edit_result.is_error,
        "edit should succeed: {edit_result:?}"
    );

    let write_result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "one\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !write_result.is_error,
        "write should trust the successful edit snapshot: {write_result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read updated fixture"),
        "one\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_can_use_permission_preview_snapshot_for_approved_update() {
    let root = temp_root("mutation-write-preview-snapshot");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);
    let cancellation = CancellationToken::new();
    let call = ToolCall::new(
        "write-1",
        "write",
        serde_json::json!({
            "path": "notes.txt",
            "content": "one\nupdated\n"
        }),
    );

    let preview = registry
        .permission_preview(&call, &cancellation)
        .expect("write approval should build a diff preview");
    assert_eq!(preview.old_text.as_deref(), Some("one\ntwo\n"));
    assert!(preview.snapshot.is_some());

    let rejected_without_approval_snapshot =
        registry.execute_tool(call.clone(), &cancellation).await;
    assert!(rejected_without_approval_snapshot.is_error);
    assert!(
        rejected_without_approval_snapshot
            .content
            .contains("has not been read")
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read unchanged fixture"),
        "one\ntwo\n"
    );

    let write_result = registry
        .execute_tool_with_context(
            call,
            ToolExecutionContext::new(&cancellation)
                .with_permission_snapshot(preview.snapshot.clone()),
        )
        .await;

    assert!(
        !write_result.is_error,
        "approved write should trust the preview snapshot: {write_result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read updated fixture"),
        "one\nupdated\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_can_use_permission_preview_snapshot_for_approved_update() {
    let root = temp_root("mutation-edit-preview-snapshot");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);
    let cancellation = CancellationToken::new();
    let call = ToolCall::new(
        "edit-1",
        "edit",
        serde_json::json!({
            "path": "notes.txt",
            "edits": [
                { "old_string": "two\n", "new_string": "updated\n" }
            ]
        }),
    );

    let preview = registry
        .permission_preview(&call, &cancellation)
        .expect("edit approval should build a diff preview");
    assert_eq!(preview.old_text.as_deref(), Some("one\ntwo\n"));
    assert!(preview.snapshot.is_some());

    let rejected_without_approval_snapshot =
        registry.execute_tool(call.clone(), &cancellation).await;
    assert!(rejected_without_approval_snapshot.is_error);
    assert!(
        rejected_without_approval_snapshot
            .content
            .contains("has not been read")
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read unchanged fixture"),
        "one\ntwo\n"
    );

    let edit_result = registry
        .execute_tool_with_context(
            call,
            ToolExecutionContext::new(&cancellation)
                .with_permission_snapshot(preview.snapshot.clone()),
        )
        .await;

    assert!(
        !edit_result.is_error,
        "approved edit should trust the preview snapshot: {edit_result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read updated fixture"),
        "one\nupdated\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_after_edit_still_rejects_external_changes() {
    let root = temp_root("mutation-write-after-edit-stale");
    fs::write(root.join("notes.txt"), "one\ntwo\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let read_result = registry
        .execute_tool(
            ToolCall::new("read-1", "read", serde_json::json!({ "path": "notes.txt" })),
            &CancellationToken::new(),
        )
        .await;
    assert!(!read_result.is_error);

    let edit_result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "two\n", "new_string": "edited\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(!edit_result.is_error);
    fs::write(root.join("notes.txt"), "external\n").expect("modify fixture outside tool");

    let write_result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "one\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(write_result.is_error);
    assert!(write_result.content.contains("modified since read"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read stale fixture"),
        "external\n"
    );
    cleanup(&root);
}

#[test]
fn builtin_edit_permission_preview_uses_actual_file_diff() {
    let root = temp_root("mutation-edit-permission-preview");
    fs::write(root.join("notes.txt"), "one\ntwo\nthree\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let preview = registry
        .permission_preview(
            &ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "two\n", "new_string": "" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .expect("edit should produce a permission preview");

    assert_eq!(preview.path, "notes.txt");
    assert_eq!(preview.old_text.as_deref(), Some("one\ntwo\nthree\n"));
    assert_eq!(preview.new_text, "one\nthree\n");
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        "one\ntwo\nthree\n",
        "permission preview must not modify the file"
    );
    cleanup(&root);
}

#[test]
fn builtin_write_permission_preview_includes_existing_text_when_updating() {
    let root = temp_root("mutation-write-permission-preview");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let preview = registry
        .permission_preview(
            &ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "notes.txt",
                    "content": "new\n"
                }),
            ),
            &CancellationToken::new(),
        )
        .expect("write should produce a permission preview");

    assert_eq!(preview.path, "notes.txt");
    assert_eq!(preview.old_text.as_deref(), Some("old\n"));
    assert_eq!(preview.new_text, "new\n");
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_permission_preview_and_result_details_are_bounded() {
    let root = temp_root("mutation-write-bounded-preview");
    let old_text = "old line\n".repeat(40_000);
    let new_text = "new line\n".repeat(40_000);
    fs::write(root.join("notes.txt"), &old_text).expect("write fixture");
    let registry = workspace_tool_registry(&root);
    let cancellation = CancellationToken::new();
    let call = ToolCall::new(
        "write-1",
        "write",
        serde_json::json!({
            "path": "notes.txt",
            "content": new_text
        }),
    );

    let preview = registry
        .permission_preview(&call, &cancellation)
        .expect("write should produce a bounded permission preview");
    assert!(preview.is_truncated);
    assert!(preview.snapshot.is_some());
    assert_ne!(preview.old_text.as_deref(), Some(old_text.as_str()));
    assert_ne!(preview.new_text, new_text);
    assert!(
        preview.old_text.as_deref().map(str::len).unwrap_or(0) + preview.new_text.len()
            <= 256 * 1024
    );
    assert!(
        preview
            .old_text
            .as_deref()
            .map(str::lines)
            .map(Iterator::count)
            .unwrap_or(0)
            + preview.new_text.lines().count()
            <= 6_000
    );

    let result = registry
        .execute_tool_with_context(
            call,
            ToolExecutionContext::new(&cancellation)
                .with_permission_snapshot(preview.snapshot.clone()),
        )
        .await;
    assert!(
        !result.is_error,
        "approved large write should succeed: {result:?}"
    );

    let details = result
        .details
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("write result should include bounded details");
    assert_eq!(
        details
            .get("preview_truncated")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_ne!(
        details.get("old_text").and_then(serde_json::Value::as_str),
        Some(old_text.as_str())
    );
    assert_ne!(
        details.get("new_text").and_then(serde_json::Value::as_str),
        Some(new_text.as_str())
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read updated fixture"),
        new_text
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_write_create_result_details_are_bounded() {
    let root = temp_root("mutation-write-create-bounded-details");
    let new_text = "new file line\n".repeat(40_000);
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "write-1",
                "write",
                serde_json::json!({
                    "path": "created.txt",
                    "content": new_text
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !result.is_error,
        "large file creation should succeed: {result:?}"
    );

    let details = result
        .details
        .as_ref()
        .and_then(serde_json::Value::as_object)
        .expect("write result should include bounded details");
    assert_eq!(
        details
            .get("preview_truncated")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_ne!(
        details.get("new_text").and_then(serde_json::Value::as_str),
        Some(new_text.as_str())
    );
    assert_eq!(
        fs::read_to_string(root.join("created.txt")).expect("read created fixture"),
        new_text
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
                    "edits": [
                        { "old_string": "beta\n", "new_string": "gamma\n" }
                    ]
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
