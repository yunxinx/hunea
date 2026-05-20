use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mo_tools::{
    ToolCall, ToolExecutor, ToolExecutorRegistry, ToolKind, ToolPermissionPolicy,
    builtin::{list_dir_tool, read_tool, workspace_readonly_tool_registry},
};
use tokio_util::sync::CancellationToken;

#[test]
fn builtin_workspace_readonly_registry_exposes_file_tools() {
    let root = temp_root("builtin-definitions");
    let registry = workspace_readonly_tool_registry(&root);
    let definitions = registry.definitions();

    assert!(definitions.definition("read").is_some());
    assert!(definitions.definition("list_dir").is_some());
    assert_eq!(
        definitions
            .definition("read")
            .map(|definition| definition.kind),
        Some(ToolKind::Read)
    );
    assert_eq!(
        definitions
            .definition("list_dir")
            .map(|definition| definition.kind),
        Some(ToolKind::Search)
    );

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

    assert!(!result.is_error);
    assert_eq!(result.content, "2\ttwo\n3\tthree");
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

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "list_dir", serde_json::json!({})),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error);
    assert!(result.content.contains("Cargo.toml"));
    assert!(result.content.contains("src/"));
    assert!(result.content.contains(".hidden"));
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

    assert!(!result.is_error);
    assert!(result.content.contains(".hidden"));
    assert!(result.content.contains("src/"));
    assert!(!result.content.contains("target/"));
    assert!(!result.content.contains("scratch.tmp"));
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

    assert!(result.is_error);
    assert!(result.content.contains("arguments do not match schema"));
    assert!(result.content.contains("recursive"));
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

    assert!(result.is_error);
    assert!(result.content.contains("arguments do not match schema"));
    assert!(result.content.contains("offset"));
    assert!(result.content.contains("minimum"));
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

    assert!(!result.is_error);
    assert!(result.content.contains("a.txt"));
    assert!(result.content.contains("b.txt"));
    assert!(!result.content.contains("c.txt"));
    assert!(result.content.contains("Truncated"));
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

    assert!(result.is_error);
    assert!(result.content.contains("outside workspace"));
    cleanup(&outside);
    cleanup(&root);
}

#[tokio::test]
async fn builtin_read_rejects_explicit_attachment_formats() {
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

    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("image/png files must be attached explicitly")
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

    assert!(result.is_error);
    assert!(result.content.contains("not valid UTF-8 text"));
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

    assert!(result.is_error);
    assert!(result.content.contains("not valid UTF-8 text"));
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
