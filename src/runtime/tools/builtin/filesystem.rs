use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::runtime::tools::{
    RuntimeTool, RuntimeToolCall, RuntimeToolDefinition, RuntimeToolExecutionFuture,
    RuntimeToolExecutorRegistry, RuntimeToolResult, ToolPermissionPolicy,
};

const FILE_READ_TOOL_NAME: &str = "file_read";
const LIST_DIR_TOOL_NAME: &str = "list_dir";
const FILE_READ_DEFAULT_LINE_COUNT: usize = 2_000;
const FILE_READ_MAX_LINE_COUNT: usize = 5_000;
const FILE_READ_MAX_LINE_CHARS: usize = 2_000;

/// `workspace_readonly_tool_registry` 创建只读 workspace 文件工具注册表。
pub fn workspace_readonly_tool_registry(root: impl AsRef<Path>) -> RuntimeToolExecutorRegistry {
    let root = root.as_ref().to_path_buf();
    let mut registry = RuntimeToolExecutorRegistry::new();
    registry.insert(FileReadTool { root: root.clone() });
    registry.insert(ListDirTool { root });
    registry
}

#[derive(Debug, Clone)]
struct FileReadTool {
    root: PathBuf,
}

impl RuntimeTool for FileReadTool {
    fn definition(&self) -> RuntimeToolDefinition {
        RuntimeToolDefinition::new(FILE_READ_TOOL_NAME)
            .with_label("File Read")
            .with_description(
                "Read a UTF-8 text file inside the current workspace, optionally by line range.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file path"
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line number to start reading from"
                    },
                    "end_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line number to stop reading at, inclusive"
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Never)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a> {
        let root = self.root.clone();
        Box::pin(async move { execute_file_read(root, call) })
    }
}

#[derive(Debug, Clone)]
struct ListDirTool {
    root: PathBuf,
}

impl RuntimeTool for ListDirTool {
    fn definition(&self) -> RuntimeToolDefinition {
        RuntimeToolDefinition::new(LIST_DIR_TOOL_NAME)
            .with_label("List Directory")
            .with_description("List immediate entries of a directory inside the current workspace.")
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute directory path"
                    },
                    "show_hidden": {
                        "type": "boolean",
                        "description": "Include entries whose names start with '.'"
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Never)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a> {
        let root = self.root.clone();
        Box::pin(async move { execute_list_dir(root, call) })
    }
}

#[derive(Debug, Deserialize)]
struct FileReadArguments {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ListDirArguments {
    path: String,
    #[serde(default)]
    show_hidden: bool,
}

fn execute_file_read(root: PathBuf, call: RuntimeToolCall) -> RuntimeToolResult {
    let arguments = match serde_json::from_value::<FileReadArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return RuntimeToolResult::error(
                call.call_id,
                format!("file_read arguments are invalid: {error}"),
            );
        }
    };

    let path = match resolve_workspace_path(&root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return RuntimeToolResult::error(call.call_id, message),
    };

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return RuntimeToolResult::error(
                call.call_id,
                format!("stat failed for '{}': {error}", arguments.path),
            );
        }
    };
    if metadata.is_dir() {
        return RuntimeToolResult::error(
            call.call_id,
            format!("'{}' is a directory, use list_dir instead", arguments.path),
        );
    }
    if !metadata.is_file() {
        return RuntimeToolResult::error(
            call.call_id,
            format!("'{}' is not a regular file", arguments.path),
        );
    }

    match read_text_file_range(&path, arguments.start_line, arguments.end_line) {
        Ok(content) => RuntimeToolResult::success(call.call_id, content),
        Err(message) => RuntimeToolResult::error(call.call_id, message),
    }
}

fn execute_list_dir(root: PathBuf, call: RuntimeToolCall) -> RuntimeToolResult {
    let arguments = match serde_json::from_value::<ListDirArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return RuntimeToolResult::error(
                call.call_id,
                format!("list_dir arguments are invalid: {error}"),
            );
        }
    };

    let path = match resolve_workspace_path(&root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return RuntimeToolResult::error(call.call_id, message),
    };

    let metadata = match fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return RuntimeToolResult::error(
                call.call_id,
                format!("stat failed for '{}': {error}", arguments.path),
            );
        }
    };
    if !metadata.is_dir() {
        return RuntimeToolResult::error(
            call.call_id,
            format!("'{}' is a file, use file_read instead", arguments.path),
        );
    }

    match list_directory_entries(&path, arguments.show_hidden) {
        Ok(content) => RuntimeToolResult::success(call.call_id, content),
        Err(message) => RuntimeToolResult::error(call.call_id, message),
    }
}

fn resolve_workspace_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("'path' is required".to_string());
    }

    let root = root
        .canonicalize()
        .map_err(|error| format!("workspace root is unavailable: {error}"))?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    let candidate = candidate
        .canonicalize()
        .map_err(|error| format!("path not found: {requested}: {error}"))?;
    if !candidate.starts_with(&root) {
        return Err(format!("path is outside workspace: {requested}"));
    }
    Ok(candidate)
}

fn read_text_file_range(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    let start_line = start_line.unwrap_or(1).max(1);
    let mut end_line = end_line.unwrap_or(start_line + FILE_READ_DEFAULT_LINE_COUNT - 1);
    if end_line < start_line {
        end_line = start_line;
    }
    if end_line - start_line + 1 > FILE_READ_MAX_LINE_COUNT {
        end_line = start_line + FILE_READ_MAX_LINE_COUNT - 1;
    }

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

fn list_directory_entries(path: &Path, show_hidden: bool) -> Result<String, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("read directory failed for '{}': {error}", path.display()))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
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
    Ok(entries.join("\n"))
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
