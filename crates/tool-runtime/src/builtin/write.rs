use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolKind, ToolPermissionPolicy, ToolResult,
};

use super::{
    file_state::WorkspaceReadState,
    mutation::{
        TOOL_CALL_INTERRUPTED, ensure_existing_file_was_read, existing_file_metadata,
        record_written_text_snapshot,
    },
    workspace::resolve_workspace_write_path,
    workspace_access::{SharedWorkspaceAccess, local_workspace_access},
};

const WRITE_TOOL_NAME: &str = "write";

/// `write_tool` 创建 workspace 文件完整写入工具。
pub fn write_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    write_tool_with_access(
        root,
        local_workspace_access(),
        WorkspaceReadState::default(),
    )
}

pub(crate) fn write_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
) -> impl Tool + 'static {
    WriteTool {
        root: root.as_ref().to_path_buf(),
        access,
        read_state,
    }
}

#[derive(Clone)]
struct WriteTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
}

impl std::fmt::Debug for WriteTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriteTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for WriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(WRITE_TOOL_NAME)
            .with_label("Write")
            .with_kind(ToolKind::Write)
            .with_description(
                "Create a new UTF-8 text file or fully rewrite an existing file inside the current workspace. Existing files must be read completely with read first; use edit for precise local changes.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file path"
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete UTF-8 text content to write"
                    }
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let access = self.access.clone();
        let read_state = self.read_state.clone();
        let call_id = call.call_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            match task::spawn_blocking(move || {
                execute_write(root, access, read_state, call, cancellation)
            })
            .await
            {
                Ok(result) => result,
                Err(error) => join_error_result(call_id, error),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct WriteArguments {
    path: String,
    content: String,
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("write task failed: {error}"))
}

fn execute_write(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    call: ToolCall,
    cancellation: CancellationToken,
) -> ToolResult {
    let arguments = match serde_json::from_value::<WriteArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("write arguments are invalid: {error}"),
            );
        }
    };

    let path = match resolve_workspace_write_path(access.as_ref(), &root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    match write_text_file(
        access.as_ref(),
        &read_state,
        &path,
        &arguments.path,
        &arguments.content,
        &cancellation,
    ) {
        Ok(WriteOutcome::Created) => ToolResult::success(
            call.call_id,
            format!(
                "File created successfully at: {} ({} bytes)",
                arguments.path,
                arguments.content.len()
            ),
        ),
        Ok(WriteOutcome::Updated) => ToolResult::success(
            call.call_id,
            format!(
                "The file {} has been updated successfully ({} bytes).",
                arguments.path,
                arguments.content.len()
            ),
        ),
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteOutcome {
    Created,
    Updated,
}

fn write_text_file(
    access: &dyn super::workspace_access::WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    requested_path: &str,
    content: &str,
    cancellation: &CancellationToken,
) -> Result<WriteOutcome, String> {
    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }

    let metadata = existing_file_metadata(access, path, requested_path, "write")?;
    if let Some(metadata) = metadata.as_ref() {
        ensure_existing_file_was_read(access, read_state, path, metadata, cancellation)?;
    }

    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }
    if let Some(parent) = path.parent() {
        access.create_dir_all(parent).map_err(|error| {
            format!(
                "create parent directory failed for '{}': {error}",
                parent.display()
            )
        })?;
    }
    access
        .write_text_file(path, content)
        .map_err(|error| format!("write failed for '{}': {error}", path.display()))?;
    record_written_text_snapshot(access, read_state, path, content);

    if metadata.is_some() {
        Ok(WriteOutcome::Updated)
    } else {
        Ok(WriteOutcome::Created)
    }
}
