use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture, ToolKind,
    ToolPermissionFileSnapshot, ToolPermissionPolicy, ToolPermissionPreview, ToolResult,
};

use super::{
    edit_apply::{EditApplication, EditRequest, TextEdit, apply_edit},
    error::WorkspaceFileError,
    file_state::WorkspaceReadState,
    mutation::{
        WorkspaceMutationQueue, bounded_permission_preview, bounded_tool_result_details,
        ensure_existing_file_was_read, existing_file_metadata, permission_file_snapshot,
        read_existing_text_file, record_written_text_snapshot,
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
        WorkspaceMutationQueue::default(),
    )
}

pub(crate) fn edit_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    mutation_queue: WorkspaceMutationQueue,
) -> impl Tool + 'static {
    EditTool {
        root: root.as_ref().to_path_buf(),
        access,
        read_state,
        mutation_queue,
    }
}

#[derive(Clone)]
struct EditTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    mutation_queue: WorkspaceMutationQueue,
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
                "Edit an existing UTF-8 text file inside the current workspace by applying one or more targeted replacements. Existing files must be read completely with read first. Each old_string is matched against the original file, not incrementally after earlier edits. Keep old_string as small as possible while still unique; merge nearby changes into one edit and avoid including large unchanged regions. Use write to create files.",
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
                        "description": "One or more targeted replacements. A single replacement is still passed as a one-item array. Each old_string is matched against the original file, must be unique after fuzzy normalization, and must not overlap another edit.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "old_string": {
                                    "type": "string",
                                    "description": "Text to replace for this targeted edit. Must not be empty, must be unique in the original file, and must not overlap with any other old_string in the same call."
                                },
                                "new_string": {
                                    "type": "string",
                                    "description": "Replacement text for this targeted edit"
                                }
                            },
                            "required": ["old_string", "new_string"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["path", "edits"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Ask)
            .with_prompt_guidelines(
                "Read a file before editing it. Use edit for targeted changes; use write for new files or full rewrites.",
            )
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let access = self.access.clone();
        let read_state = self.read_state.clone();
        let mutation_queue = self.mutation_queue.clone();
        let call_id = call.call_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            match task::spawn_blocking(move || {
                execute_edit(
                    root,
                    access,
                    read_state,
                    mutation_queue,
                    call,
                    None,
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

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let access = self.access.clone();
        let read_state = self.read_state.clone();
        let mutation_queue = self.mutation_queue.clone();
        let permission_snapshot = context.permission_snapshot().cloned();
        let call_id = call.call_id.clone();
        let cancellation = context.cancellation().clone();
        Box::pin(async move {
            match task::spawn_blocking(move || {
                execute_edit(
                    root,
                    access,
                    read_state,
                    mutation_queue,
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
        edit_permission_preview(
            &self.root,
            self.access.as_ref(),
            call.arguments.clone(),
            cancellation,
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditArguments {
    path: String,
    edits: Vec<EditItemArgument>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditItemArgument {
    old_string: String,
    new_string: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum EditRequestError {
    #[error("edits must contain at least one replacement")]
    EmptyEdits,
    #[error("edits[{index}].old_string must not be empty")]
    EmptyOldString { index: usize },
}

impl EditArguments {
    fn into_request(self) -> Result<(String, EditRequest), EditRequestError> {
        if self.edits.is_empty() {
            return Err(EditRequestError::EmptyEdits);
        }
        let edits = self
            .edits
            .into_iter()
            .enumerate()
            .map(|(index, edit)| {
                if edit.old_string.is_empty() {
                    return Err(EditRequestError::EmptyOldString { index });
                }
                Ok(TextEdit {
                    old_string: edit.old_string,
                    new_string: edit.new_string,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok((self.path, EditRequest { edits }))
    }
}

#[cfg(test)]
mod argument_tests {
    use super::{EditArguments, EditItemArgument, EditRequestError};

    #[test]
    fn edit_arguments_reject_empty_old_string_with_indexed_error() {
        let arguments = EditArguments {
            path: "notes.txt".to_string(),
            edits: vec![EditItemArgument {
                old_string: String::new(),
                new_string: "replacement".to_string(),
            }],
        };

        let error = arguments
            .into_request()
            .expect_err("empty old_string should be rejected");

        assert!(matches!(
            error,
            EditRequestError::EmptyOldString { index: 0 }
        ));
    }
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("edit task failed: {error}"))
}

fn execute_edit(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    mutation_queue: WorkspaceMutationQueue,
    call: ToolCall,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
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
        Err(error) => return ToolResult::error(call.call_id, error.to_string()),
    };

    match edit_text_file(EditTextFileOptions {
        access: access.as_ref(),
        read_state: &read_state,
        path: &path,
        requested_path: &requested_path,
        request: &request,
        permission_snapshot: permission_snapshot.as_ref(),
        cancellation: &cancellation,
        mutation_queue: &mutation_queue,
    }) {
        Ok(EditOutcome::Updated {
            old_text,
            new_text,
            replacements,
        }) => edit_success_result(EditSuccessResultOptions {
            call_id: call.call_id,
            requested_path,
            old_text: Some(old_text),
            new_text,
            replacements,
        }),
        Err(error) => ToolResult::error(call.call_id, error.to_string()),
    }
}

fn edit_permission_preview(
    root: &Path,
    access: &dyn WorkspaceAccess,
    arguments: serde_json::Value,
    cancellation: &CancellationToken,
) -> Option<ToolPermissionPreview> {
    if cancellation.is_cancelled() {
        return None;
    }
    let arguments = serde_json::from_value::<EditArguments>(arguments).ok()?;
    let (requested_path, request) = arguments.into_request().ok()?;
    let path = resolve_workspace_write_path(access, root, &requested_path).ok()?;
    let metadata = existing_file_metadata(access, &path, &requested_path, "edit").ok()?;
    let metadata = metadata?;

    let original = read_existing_text_file(access, &path).ok()?;
    let application = apply_edit(&original, &request, &requested_path).ok()?;
    let snapshot = Some(permission_file_snapshot(&metadata, &original));
    Some(bounded_permission_preview(
        requested_path,
        Some(original),
        application.final_content,
        snapshot,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditOutcome {
    Updated {
        old_text: String,
        new_text: String,
        replacements: usize,
    },
}

struct EditSuccessResultOptions {
    call_id: String,
    requested_path: String,
    old_text: Option<String>,
    new_text: String,
    replacements: usize,
}

fn edit_success_result(options: EditSuccessResultOptions) -> ToolResult {
    let EditSuccessResultOptions {
        call_id,
        requested_path,
        old_text,
        new_text,
        replacements,
    } = options;
    let mut details = serde_json::Map::new();
    let bounded_details = bounded_tool_result_details(requested_path.clone(), old_text, new_text);
    if let Some(bounded_details) = bounded_details.as_object() {
        details.extend(bounded_details.clone());
    }
    details.insert("replacements".to_string(), json!(replacements));

    ToolResult::success(
        call_id,
        format!("Successfully replaced {replacements} block(s) in {requested_path}."),
    )
    .with_details(Value::Object(details))
}

struct EditTextFileOptions<'a> {
    access: &'a dyn WorkspaceAccess,
    read_state: &'a WorkspaceReadState,
    path: &'a Path,
    requested_path: &'a str,
    request: &'a EditRequest,
    permission_snapshot: Option<&'a ToolPermissionFileSnapshot>,
    cancellation: &'a CancellationToken,
    mutation_queue: &'a WorkspaceMutationQueue,
}

fn edit_text_file(options: EditTextFileOptions<'_>) -> Result<EditOutcome, WorkspaceFileError> {
    let EditTextFileOptions {
        access,
        read_state,
        path,
        requested_path,
        request,
        permission_snapshot,
        cancellation,
        mutation_queue,
    } = options;

    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }

    let _mutation_guard = mutation_queue.lock_path(path);
    let metadata = existing_file_metadata(access, path, requested_path, "edit")?;
    let Some(metadata) = metadata else {
        return Err(WorkspaceFileError::MissingEditTarget {
            path: requested_path.to_string(),
        });
    };

    ensure_existing_file_was_read(
        access,
        read_state,
        permission_snapshot,
        path,
        &metadata,
        cancellation,
    )?;
    let original = read_existing_text_file(access, path)?;
    let EditApplication {
        final_content,
        replacements,
    } = apply_edit(&original, request, requested_path)
        .map_err(|source| WorkspaceFileError::EditRejected { source })?;

    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    access
        .write_text_file(path, &final_content)
        .map_err(|source| WorkspaceFileError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    record_written_text_snapshot(access, read_state, path, &final_content);

    Ok(EditOutcome::Updated {
        old_text: original,
        new_text: final_content,
        replacements,
    })
}
