use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolKind, ToolPermissionPolicy, ToolResult,
};

use super::workspace::resolve_workspace_path;

const LIST_DIR_TOOL_NAME: &str = "list_dir";
const LIST_DIR_DEFAULT_ENTRY_LIMIT: usize = 500;
const LIST_DIR_MAX_ENTRY_LIMIT: usize = 2_000;

/// `list_dir_tool` 创建只读 workspace 目录列举工具。
pub fn list_dir_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    ListDirTool {
        root: root.as_ref().to_path_buf(),
    }
}

#[derive(Debug, Clone)]
struct ListDirTool {
    root: PathBuf,
}

impl Tool for ListDirTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(LIST_DIR_TOOL_NAME)
            .with_label("List Directory")
            .with_kind(ToolKind::Search)
            .with_description(
                "List immediate entries of a directory inside the current workspace. Entries are sorted alphabetically, include dotfiles, and directories end with '/'.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute directory path; defaults to the workspace root"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": LIST_DIR_MAX_ENTRY_LIMIT,
                        "description": "Maximum number of entries to return"
                    }
                },
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        Box::pin(async move { execute_list_dir(root, call) })
    }
}

#[derive(Debug, Deserialize)]
struct ListDirArguments {
    path: Option<String>,
    limit: Option<usize>,
}

fn execute_list_dir(root: PathBuf, call: ToolCall) -> ToolResult {
    let arguments = match serde_json::from_value::<ListDirArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("list_dir arguments are invalid: {error}"),
            );
        }
    };
    let requested_path = arguments.path.as_deref().unwrap_or(".");

    let path = match resolve_workspace_path(&root, requested_path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("stat failed for '{requested_path}': {error}"),
            );
        }
    };
    if !metadata.is_dir() {
        return ToolResult::error(
            call.call_id,
            format!("'{requested_path}' is a file, use file_read instead"),
        );
    }

    let limit = arguments
        .limit
        .unwrap_or(LIST_DIR_DEFAULT_ENTRY_LIMIT)
        .clamp(1, LIST_DIR_MAX_ENTRY_LIMIT);
    match list_directory_entries(&path, limit) {
        Ok(content) => ToolResult::success(call.call_id, content),
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

fn list_directory_entries(path: &Path, limit: usize) -> Result<String, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("read directory failed for '{}': {error}", path.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().ok()?;
            let display = if file_type.is_dir() {
                format!("{name}/")
            } else {
                name
            };
            Some(display)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.to_lowercase());

    if entries.is_empty() {
        return Ok("No entries found.".to_string());
    }

    let total_entries = entries.len();
    let mut content = entries
        .into_iter()
        .take(limit)
        .collect::<Vec<_>>()
        .join("\n");
    if total_entries > limit {
        let next_limit = limit.saturating_mul(2).min(LIST_DIR_MAX_ENTRY_LIMIT);
        if next_limit > limit {
            content.push_str(&format!(
                "\n\n[Truncated: showing {limit} of {total_entries} entries. Use limit={next_limit} for more.]"
            ));
        } else {
            content.push_str(&format!(
                "\n\n[Truncated: showing {limit} of {total_entries} entries. Maximum limit is {LIST_DIR_MAX_ENTRY_LIMIT}.]"
            ));
        }
    }
    Ok(content)
}
