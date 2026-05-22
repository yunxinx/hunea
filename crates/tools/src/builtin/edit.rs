use std::{
    io::Read,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolKind, ToolPermissionPolicy, ToolResult,
};

use super::{
    edit_apply::{EditApplication, EditRequest, TextEdit, apply_edit},
    file_state::WorkspaceReadState,
    mutation::{
        TOOL_CALL_INTERRUPTED, ensure_existing_file_was_read, existing_file_metadata,
        record_written_text_snapshot,
    },
    workspace::resolve_workspace_write_path,
    workspace_access::{SharedWorkspaceAccess, WorkspaceAccess, local_workspace_access},
};

const EDIT_TOOL_NAME: &str = "edit";

/// `edit_tool` 创建 workspace 文件局部替换工具。
pub fn edit_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    edit_tool_with_access(
        root,
        local_workspace_access(),
        WorkspaceReadState::default(),
    )
}

pub(crate) fn edit_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
) -> impl Tool + 'static {
    EditTool {
        root: root.as_ref().to_path_buf(),
        access,
        read_state,
    }
}

#[derive(Clone)]
struct EditTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
}

impl std::fmt::Debug for EditTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EditTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for EditTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(EDIT_TOOL_NAME)
            .with_label("Edit")
            .with_kind(ToolKind::Edit)
            .with_description(
                "Edit a UTF-8 text file inside the current workspace by replacing targeted text. Existing files must be read completely with read first. Use edits for multiple disjoint replacements in one call. If old_string is empty and the file does not exist, edit creates the file with new_string.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file path"
                    },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "description": "One or more targeted replacements. Each old_string is matched against the original file, must be unique after fuzzy normalization, and must not overlap another edit. Do not combine edits with old_string/new_string or replace_all.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": {
                                    "type": "string",
                                    "description": "Text to replace for this targeted edit. Must not be empty."
                                },
                                "new_string": {
                                    "type": "string",
                                    "description": "Replacement text for this targeted edit"
                                }
                            },
                            "required": ["old_string", "new_string"],
                            "additionalProperties": false
                        }
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text to replace for a single replacement. Use an empty string only to create a missing file."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement text"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Whether to replace every occurrence of old_string; defaults to false"
                    }
                },
                "required": ["path"],
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
                execute_edit(root, access, read_state, call, cancellation)
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
#[serde(deny_unknown_fields)]
struct EditArguments {
    path: String,
    old_string: Option<String>,
    new_string: Option<String>,
    replace_all: Option<bool>,
    edits: Option<Vec<EditItemArgument>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditItemArgument {
    old_string: String,
    new_string: String,
}

impl EditArguments {
    fn into_request(self) -> Result<(String, EditRequest), String> {
        let has_single_edit = self.old_string.is_some() || self.new_string.is_some();
        let has_edits = self.edits.is_some();

        if has_single_edit && has_edits {
            return Err("provide either edits or old_string/new_string, not both".to_string());
        }

        if let Some(edits) = self.edits {
            if self.replace_all.is_some() {
                return Err("replace_all is only supported with old_string/new_string".to_string());
            }
            if edits.is_empty() {
                return Err("edits must contain at least one replacement".to_string());
            }
            let edits = edits
                .into_iter()
                .enumerate()
                .map(|(index, edit)| {
                    if edit.old_string.is_empty() {
                        return Err(format!("edits[{index}].old_string must not be empty"));
                    }
                    Ok(TextEdit {
                        old_string: edit.old_string,
                        new_string: edit.new_string,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;

            return Ok((self.path, EditRequest::Multiple { edits }));
        }

        let old_string = self
            .old_string
            .ok_or_else(|| "old_string is required when edits is not provided".to_string())?;
        let new_string = self
            .new_string
            .ok_or_else(|| "new_string is required when edits is not provided".to_string())?;

        Ok((
            self.path,
            EditRequest::Single {
                edit: TextEdit {
                    old_string,
                    new_string,
                },
                replace_all: self.replace_all.unwrap_or(false),
            },
        ))
    }
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("edit task failed: {error}"))
}

fn execute_edit(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    call: ToolCall,
    cancellation: CancellationToken,
) -> ToolResult {
    let arguments = match serde_json::from_value::<EditArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(call.call_id, format!("edit arguments are invalid: {error}"));
        }
    };

    let (requested_path, request) = match arguments.into_request() {
        Ok(request) => request,
        Err(error) => {
            return ToolResult::error(call.call_id, format!("edit arguments are invalid: {error}"));
        }
    };

    let path = match resolve_workspace_write_path(access.as_ref(), &root, &requested_path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    match edit_text_file(EditTextFileOptions {
        access: access.as_ref(),
        read_state: &read_state,
        path: &path,
        requested_path: &requested_path,
        request: &request,
        cancellation: &cancellation,
    }) {
        Ok(EditOutcome::Created) => ToolResult::success(
            call.call_id,
            format!("File created successfully at: {requested_path}"),
        ),
        Ok(EditOutcome::Updated { replacements }) => ToolResult::success(
            call.call_id,
            format!(
                "The file {} has been updated successfully. Replaced {replacements} occurrence(s).",
                requested_path
            ),
        ),
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditOutcome {
    Created,
    Updated { replacements: usize },
}

struct EditTextFileOptions<'a> {
    access: &'a dyn WorkspaceAccess,
    read_state: &'a WorkspaceReadState,
    path: &'a Path,
    requested_path: &'a str,
    request: &'a EditRequest,
    cancellation: &'a CancellationToken,
}

fn edit_text_file(options: EditTextFileOptions<'_>) -> Result<EditOutcome, String> {
    let EditTextFileOptions {
        access,
        read_state,
        path,
        requested_path,
        request,
        cancellation,
    } = options;

    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }

    let metadata = existing_file_metadata(access, path, requested_path, "edit")?;
    let Some(metadata) = metadata else {
        match request {
            EditRequest::Single { edit, .. } if edit.old_string.is_empty() => {
                write_new_file(access, read_state, path, &edit.new_string)?;
                return Ok(EditOutcome::Created);
            }
            EditRequest::Single { .. } => {
                return Err(format!(
                    "File does not exist. To create a new file with edit, set old_string to an empty string: {requested_path}"
                ));
            }
            EditRequest::Multiple { .. } => {
                return Err(format!(
                    "File does not exist. Use write to create a new file before applying edits: {requested_path}"
                ));
            }
        }
    };

    if let EditRequest::Single { edit, .. } = request
        && edit.old_string == edit.new_string
    {
        return Err(
            "No changes to make: old_string and new_string are exactly the same.".to_string(),
        );
    }

    ensure_existing_file_was_read(access, read_state, path, &metadata, cancellation)?;
    let original = read_existing_text_file(access, path)?;
    let EditApplication {
        final_content,
        replacements,
    } = apply_edit(&original, request, requested_path)?;

    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }
    access
        .write_text_file(path, &final_content)
        .map_err(|error| format!("edit failed for '{}': {error}", path.display()))?;
    record_written_text_snapshot(access, read_state, path, &final_content);

    Ok(EditOutcome::Updated { replacements })
}

fn write_new_file(
    access: &dyn WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    content: &str,
) -> Result<(), String> {
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
        .map_err(|error| format!("edit failed for '{}': {error}", path.display()))?;
    record_written_text_snapshot(access, read_state, path, content);
    Ok(())
}

fn read_existing_text_file(access: &dyn WorkspaceAccess, path: &Path) -> Result<String, String> {
    let mut reader = access
        .open_reader(path)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| {
        format!(
            "edit failed for '{}': file is not valid UTF-8 text",
            path.display()
        )
    })
}
