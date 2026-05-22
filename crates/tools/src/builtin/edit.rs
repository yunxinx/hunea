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
    file_state::WorkspaceReadState,
    mutation::{
        TOOL_CALL_INTERRUPTED, ensure_existing_file_was_read, existing_file_metadata,
        record_written_text_snapshot,
    },
    workspace::resolve_workspace_write_path,
    workspace_access::{SharedWorkspaceAccess, local_workspace_access},
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
                "Edit a UTF-8 text file inside the current workspace by replacing exact text. Existing files must be read completely with read first. If old_string is empty and the file does not exist, edit creates the file with new_string.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute file path"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact text to replace. Use an empty string only to create a missing file."
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
                "required": ["path", "old_string", "new_string"],
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
struct EditArguments {
    path: String,
    old_string: String,
    new_string: String,
    replace_all: Option<bool>,
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

    let path = match resolve_workspace_write_path(access.as_ref(), &root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    let replace_all = arguments.replace_all.unwrap_or(false);
    match edit_text_file(EditTextFileOptions {
        access: access.as_ref(),
        read_state: &read_state,
        path: &path,
        requested_path: &arguments.path,
        old_string: &arguments.old_string,
        new_string: &arguments.new_string,
        replace_all,
        cancellation: &cancellation,
    }) {
        Ok(EditOutcome::Created) => ToolResult::success(
            call.call_id,
            format!("File created successfully at: {}", arguments.path),
        ),
        Ok(EditOutcome::Updated { replacements }) => ToolResult::success(
            call.call_id,
            format!(
                "The file {} has been updated successfully. Replaced {replacements} occurrence(s).",
                arguments.path
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
    access: &'a dyn super::workspace_access::WorkspaceAccess,
    read_state: &'a WorkspaceReadState,
    path: &'a Path,
    requested_path: &'a str,
    old_string: &'a str,
    new_string: &'a str,
    replace_all: bool,
    cancellation: &'a CancellationToken,
}

fn edit_text_file(options: EditTextFileOptions<'_>) -> Result<EditOutcome, String> {
    let EditTextFileOptions {
        access,
        read_state,
        path,
        requested_path,
        old_string,
        new_string,
        replace_all,
        cancellation,
    } = options;

    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }

    let metadata = existing_file_metadata(access, path, requested_path, "edit")?;
    let Some(metadata) = metadata else {
        if !old_string.is_empty() {
            return Err(format!(
                "File does not exist. To create a new file with edit, set old_string to an empty string: {requested_path}"
            ));
        }
        write_new_file(access, read_state, path, new_string)?;
        return Ok(EditOutcome::Created);
    };

    if old_string == new_string {
        return Err(
            "No changes to make: old_string and new_string are exactly the same.".to_string(),
        );
    }

    ensure_existing_file_was_read(access, read_state, path, &metadata, cancellation)?;
    let original = read_existing_text_file(access, path)?;
    let EditApplication {
        final_content,
        replacements,
    } = apply_edit(
        &original,
        old_string,
        new_string,
        replace_all,
        requested_path,
    )?;

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
    access: &dyn super::workspace_access::WorkspaceAccess,
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

fn read_existing_text_file(
    access: &dyn super::workspace_access::WorkspaceAccess,
    path: &Path,
) -> Result<String, String> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditApplication {
    final_content: String,
    replacements: usize,
}

fn apply_edit(
    original: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
    requested_path: &str,
) -> Result<EditApplication, String> {
    let (bom, content_without_bom) = strip_utf8_bom(original);
    let line_ending = detect_line_ending(content_without_bom);
    let normalized_content = normalize_line_endings(content_without_bom);
    let normalized_old = normalize_line_endings(old_string);
    let normalized_new = normalize_line_endings(new_string);

    if normalized_old.is_empty() {
        if !normalized_content.is_empty() {
            return Err("Cannot create new file - file already exists.".to_string());
        }
        return Ok(EditApplication {
            final_content: format!(
                "{bom}{}",
                restore_line_endings(&normalized_new, line_ending)
            ),
            replacements: 1,
        });
    }

    let matches = normalized_content.matches(&normalized_old).count();
    if matches == 0 {
        return Err(format!(
            "String to replace not found in file.\nString: {old_string}"
        ));
    }
    if matches > 1 && !replace_all {
        return Err(format!(
            "Found {matches} matches of the string to replace, but replace_all is false. To replace all occurrences, set replace_all to true. To replace only one occurrence, provide more context.\nString: {old_string}"
        ));
    }

    let updated = if replace_all {
        normalized_content.replace(&normalized_old, &normalized_new)
    } else {
        normalized_content.replacen(&normalized_old, &normalized_new, 1)
    };
    if updated == normalized_content {
        return Err(format!("No changes made to {requested_path}."));
    }

    Ok(EditApplication {
        final_content: format!("{bom}{}", restore_line_endings(&updated, line_ending)),
        replacements: if replace_all { matches } else { 1 },
    })
}

fn strip_utf8_bom(text: &str) -> (&str, &str) {
    text.strip_prefix('\u{feff}')
        .map(|rest| ("\u{feff}", rest))
        .unwrap_or(("", text))
}

fn detect_line_ending(text: &str) -> LineEnding {
    match (text.find("\r\n"), text.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => LineEnding::CrLf,
        _ => LineEnding::Lf,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
}

fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(text: &str, line_ending: LineEnding) -> String {
    match line_ending {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
    }
}
