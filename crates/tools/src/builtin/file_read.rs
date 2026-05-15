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

const FILE_READ_TOOL_NAME: &str = "file_read";
const FILE_READ_DEFAULT_LINE_COUNT: usize = 2_000;
const FILE_READ_MAX_LINE_COUNT: usize = 5_000;
const FILE_READ_MAX_LINE_CHARS: usize = 2_000;

/// `file_read_tool` 创建只读 workspace 文件读取工具。
pub fn file_read_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    FileReadTool {
        root: root.as_ref().to_path_buf(),
    }
}

#[derive(Debug, Clone)]
struct FileReadTool {
    root: PathBuf,
}

impl Tool for FileReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(FILE_READ_TOOL_NAME)
            .with_label("Read")
            .with_kind(ToolKind::Read)
            .with_description(
                "Read a UTF-8 text file inside the current workspace. Use offset and limit to read large files in chunks; output includes 1-based line numbers.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file path"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line number to start reading from"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": FILE_READ_MAX_LINE_COUNT,
                        "description": "Maximum number of lines to return"
                    }
                },
                "required": ["path"],
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
        Box::pin(async move { execute_file_read(root, call) })
    }
}

#[derive(Debug, Deserialize)]
struct FileReadArguments {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

fn execute_file_read(root: PathBuf, call: ToolCall) -> ToolResult {
    let arguments = match serde_json::from_value::<FileReadArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("file_read arguments are invalid: {error}"),
            );
        }
    };

    let path = match resolve_workspace_path(&root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("stat failed for '{}': {error}", arguments.path),
            );
        }
    };
    if metadata.is_dir() {
        return ToolResult::error(
            call.call_id,
            format!("'{}' is a directory, use list_dir instead", arguments.path),
        );
    }
    if !metadata.is_file() {
        return ToolResult::error(
            call.call_id,
            format!("'{}' is not a regular file", arguments.path),
        );
    }

    match read_text_file_lines(&path, arguments.offset, arguments.limit) {
        Ok(content) => ToolResult::success(call.call_id, content),
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

fn read_text_file_lines(
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    let start_line = offset.unwrap_or(1).max(1);
    let line_limit = limit
        .unwrap_or(FILE_READ_DEFAULT_LINE_COUNT)
        .clamp(1, FILE_READ_MAX_LINE_COUNT);
    let end_line = start_line.saturating_add(line_limit.saturating_sub(1));

    let mut selected = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        if line_number < start_line {
            continue;
        }
        if line_number > end_line {
            break;
        }
        selected.push(format!(
            "{line_number}\t{}",
            truncate_line(line, FILE_READ_MAX_LINE_CHARS)
        ));
    }
    Ok(selected.join("\n"))
}

fn truncate_line(line: &str, max_chars: usize) -> String {
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}
