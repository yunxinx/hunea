use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ToolCall, ToolExecutor, ToolExecutorRegistry, builtin::workspace_tool_registry,
};

#[test]
fn builtin_edit_schema_only_exposes_edits_array() {
    let root = temp_root("edit-schema-edits-only");
    let registry = workspace_tool_registry(&root);
    let definitions = registry.definitions();

    let edit_schema_properties = definitions
        .definition("edit")
        .and_then(|definition| definition.input_schema.as_ref())
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
        .expect("edit schema should expose object properties");

    assert_eq!(edit_schema_properties.len(), 2);
    assert!(edit_schema_properties.contains_key("path"));
    assert!(edit_schema_properties.contains_key("edits"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_single_edit_arguments() {
    let root = temp_root("edit-reject-single-arguments");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "old_string": "old",
                    "new_string": "new"
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("$.edits"));
    assert!(result.content().contains("is required"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_existing_file_requires_complete_prior_read() {
    let root = temp_root("edit-requires-read");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "old", "new_string": "new" }
                    ]
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
async fn builtin_edit_rejects_file_changed_after_read() {
    let root = temp_root("edit-stale");
    fs::write(root.join("notes.txt"), "old\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);

    read_complete_file(&registry, "notes.txt").await;
    fs::write(root.join("notes.txt"), "external\n").expect("modify fixture outside tool");

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "old", "new_string": "new" }
                    ]
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
async fn builtin_edit_requires_unique_match_for_single_edits_array_item() {
    let root = temp_root("edit-unique-array-item");
    fs::write(root.join("notes.txt"), "apple\napple\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "apple", "new_string": "orange" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;
    assert!(result.is_error());
    assert!(
        result.content().contains("Found 2 matches"),
        "unexpected duplicate-match error: {}",
        result.content()
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read edited fixture"),
        "apple\napple\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_accepts_multiple_disjoint_edits_in_one_call() {
    let root = temp_root("edit-multi");
    fs::write(root.join("notes.txt"), "alpha\nbeta\ngamma\ndelta\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "alpha\n", "new_string": "ALPHA\n" },
                        { "old_string": "gamma\n", "new_string": "GAMMA\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(!result.is_error(), "multi edit should succeed: {result:?}");
    assert!(
        result
            .content()
            .contains("Successfully replaced 2 block(s)")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("path"))
            .and_then(serde_json::Value::as_str),
        Some("notes.txt")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("old_text"))
            .and_then(serde_json::Value::as_str),
        Some("alpha\nbeta\ngamma\ndelta\n")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("new_text"))
            .and_then(serde_json::Value::as_str),
        Some("ALPHA\nbeta\nGAMMA\ndelta\n")
    );
    assert_eq!(
        result
            .details()
            .and_then(|details| details.get("replacements"))
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read edited fixture"),
        "ALPHA\nbeta\nGAMMA\ndelta\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_matches_multiple_edits_against_original_file() {
    let root = temp_root("edit-multi-original");
    fs::write(root.join("notes.txt"), "foo\nbar\nbaz\n").expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "foo\n", "new_string": "foo bar\n" },
                        { "old_string": "bar\n", "new_string": "BAR\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "multi edit should match original file: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read edited fixture"),
        "foo bar\nBAR\nbaz\n"
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_overlapping_edits_without_modifying_file() {
    let root = temp_root("edit-multi-overlap");
    let original = "one\ntwo\nthree\n";
    fs::write(root.join("notes.txt"), original).expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "one\ntwo\n", "new_string": "ONE\nTWO\n" },
                        { "old_string": "two\nthree\n", "new_string": "TWO\nTHREE\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("overlap"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        original
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_does_not_partially_write_when_one_multi_edit_fails() {
    let root = temp_root("edit-multi-no-partial");
    let original = "alpha\nbeta\ngamma\n";
    fs::write(root.join("notes.txt"), original).expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "alpha\n", "new_string": "ALPHA\n" },
                        { "old_string": "missing\n", "new_string": "MISSING\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("not found"));
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read fixture"),
        original
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_fuzzy_matches_common_model_text_differences() {
    let root = temp_root("edit-fuzzy");
    fs::write(
        root.join("notes.txt"),
        concat!(
            "line one   \n",
            "line two  \n",
            "const msg = \u{201c}Hello\u{201d}\u{2014}world\n",
            "hello\u{00a0}world\n",
            "\u{4f60}\u{597d}\u{ff0c}\u{4e16}\u{754c}\n",
            "\u{ff21}\u{ff22}\u{ff23}\u{ff11}\u{ff12}\u{ff13}\n",
            "cafe\u{0301}\n",
        ),
    )
    .expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "line one\nline two\n", "new_string": "trimmed\n" },
                        { "old_string": "const msg = \"Hello\"-world\n", "new_string": "const msg = \"Goodbye\"-world\n" },
                        { "old_string": "hello world\n", "new_string": "hello universe\n" },
                        { "old_string": "你好,世界\n", "new_string": "你好，hunea\n" },
                        { "old_string": "ABC123\ncafé\n", "new_string": "XYZ789\ncoffee\n" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(
        !result.is_error(),
        "fuzzy multi edit should succeed: {result:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).expect("read edited fixture"),
        concat!(
            "trimmed\n",
            "const msg = \"Goodbye\"-world\n",
            "hello universe\n",
            "你好，hunea\n",
            "XYZ789\n",
            "coffee\n",
        )
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_detects_duplicates_after_fuzzy_normalization() {
    let root = temp_root("edit-fuzzy-duplicates");
    fs::write(
        root.join("notes.txt"),
        "hello world   \nhello\u{00a0}world\n",
    )
    .expect("write fixture");
    let registry = workspace_tool_registry(&root);
    read_complete_file(&registry, "notes.txt").await;

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "hello world", "new_string": "replaced" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result.content().contains("Found 2 matches"),
        "unexpected duplicate error: {}",
        result.content()
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_replace_all_argument() {
    let root = temp_root("edit-reject-replace-all");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "replace_all": true,
                    "edits": [
                        { "old_string": "old", "new_string": "new" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("replace_all"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_empty_edits_array() {
    let root = temp_root("edit-empty-edits");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": []
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result
            .content()
            .contains("edits must contain at least one replacement")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_empty_old_string_inside_edits() {
    let root = temp_root("edit-empty-multi-old-string");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "", "new_string": "new" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(
        result
            .content()
            .contains("edits[0].old_string must not be empty")
    );
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_missing_field_inside_edits_items() {
    let root = temp_root("edit-missing-item-field");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        { "old_string": "old" }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("$.edits[0].new_string"));
    assert!(result.content().contains("is required"));
    cleanup(&root);
}

#[tokio::test]
async fn builtin_edit_rejects_extra_fields_inside_edits_items() {
    let root = temp_root("edit-extra-item-field");
    let registry = workspace_tool_registry(&root);

    let result = registry
        .execute_tool(
            ToolCall::new(
                "edit-1",
                "edit",
                serde_json::json!({
                    "path": "notes.txt",
                    "edits": [
                        {
                            "old_string": "old",
                            "new_string": "new",
                            "replace_all": true
                        }
                    ]
                }),
            ),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error());
    assert!(result.content().contains("arguments do not match schema"));
    assert!(result.content().contains("$.edits[0].replace_all"));
    assert!(result.content().contains("replace_all"));
    cleanup(&root);
}

async fn read_complete_file(registry: &ToolExecutorRegistry, path: &str) {
    let result = registry
        .execute_tool(
            ToolCall::new("read-fixture", "read", serde_json::json!({ "path": path })),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        !result.is_error(),
        "fixture file should be readable before mutation: {result:?}"
    );
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
