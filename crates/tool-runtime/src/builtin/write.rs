use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionFileSnapshot, ToolPermissionPolicy, ToolPermissionPreview, ToolResult,
};

use super::{
    file_state::WorkspaceReadState,
    mutation::{
        TOOL_CALL_INTERRUPTED, bounded_permission_preview, bounded_tool_result_details,
        ensure_existing_file_was_read, existing_file_metadata, permission_file_snapshot,
        read_existing_text_file, record_written_text_snapshot,
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
                execute_write(root, access, read_state, call, None, cancellation)
            })
            .await
            {
                Ok(result) => result,
                Err(error) => join_error_result(call_id, error),
            }
        })
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let access = self.access.clone();
        let read_state = self.read_state.clone();
        let permission_snapshot = context.permission_snapshot().cloned();
        let call_id = call.call_id.clone();
        let cancellation = context.cancellation().clone();
        Box::pin(async move {
            match task::spawn_blocking(move || {
                execute_write(
                    root,
                    access,
                    read_state,
                    call,
                    permission_snapshot,
                    cancellation,
                )
            })
            .await
            {
                Ok(result) => result,
                Err(error) => join_error_result(call_id, error),
            }
        })
    }

    fn permission_preview(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        write_permission_preview(
            &self.root,
            self.access.as_ref(),
            call.arguments.clone(),
            cancellation,
        )
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
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
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
        permission_snapshot.as_ref(),
        &cancellation,
    ) {
        Ok(WriteOutcome::Created { new_text }) => ToolResult::success(
            call.call_id,
            format!(
                "File created successfully at: {} ({} bytes)",
                arguments.path,
                arguments.content.len()
            ),
        )
        .with_details(bounded_tool_result_details(arguments.path, None, new_text)),
        Ok(WriteOutcome::Updated { old_text, new_text }) => ToolResult::success(
            call.call_id,
            format!(
                "The file {} has been updated successfully ({} bytes).",
                arguments.path,
                arguments.content.len()
            ),
        )
        .with_details(bounded_tool_result_details(
            arguments.path,
            Some(old_text),
            new_text,
        )),
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

fn write_permission_preview(
    root: &Path,
    access: &dyn super::workspace_access::WorkspaceAccess,
    arguments: serde_json::Value,
    cancellation: &CancellationToken,
) -> Option<ToolPermissionPreview> {
    if cancellation.is_cancelled() {
        return None;
    }
    let arguments = serde_json::from_value::<WriteArguments>(arguments).ok()?;
    let path = resolve_workspace_write_path(access, root, &arguments.path).ok()?;
    let metadata = existing_file_metadata(access, &path, &arguments.path, "write").ok()?;
    let old_text = metadata
        .as_ref()
        .map(|_| read_existing_text_file(access, &path))
        .transpose()
        .ok()?;
    let snapshot = metadata
        .as_ref()
        .zip(old_text.as_deref())
        .map(|(metadata, old_text)| permission_file_snapshot(metadata, old_text));
    Some(bounded_permission_preview(
        arguments.path,
        old_text,
        arguments.content,
        snapshot,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WriteOutcome {
    Created { new_text: String },
    Updated { old_text: String, new_text: String },
}

fn write_text_file(
    access: &dyn super::workspace_access::WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    requested_path: &str,
    content: &str,
    permission_snapshot: Option<&ToolPermissionFileSnapshot>,
    cancellation: &CancellationToken,
) -> Result<WriteOutcome, String> {
    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }

    let metadata = existing_file_metadata(access, path, requested_path, "write")?;
    if let Some(metadata) = metadata.as_ref() {
        ensure_existing_file_was_read(
            access,
            read_state,
            permission_snapshot,
            path,
            metadata,
            cancellation,
        )?;
    }
    let old_text = metadata
        .as_ref()
        .map(|_| read_existing_text_file(access, path))
        .transpose()?;

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

    if let Some(old_text) = old_text {
        Ok(WriteOutcome::Updated {
            old_text,
            new_text: content.to_string(),
        })
    } else {
        Ok(WriteOutcome::Created {
            new_text: content.to_string(),
        })
    }
}
