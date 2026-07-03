use std::{
    io::{BufRead, BufReader},
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
    file_state::{
        TextFingerprint, TextFingerprintBuilder, WorkspaceFileSnapshot, WorkspaceReadState,
    },
    workspace::resolve_workspace_path,
    workspace_access::{SharedWorkspaceAccess, WorkspaceAccess, local_workspace_access},
};

const READ_TOOL_NAME: &str = "read";
const READ_DEFAULT_LINE_COUNT: usize = 2_000;
const READ_MAX_LINE_COUNT: usize = 5_000;
const READ_MAX_LINE_CHARS: usize = 2_000;
const READ_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const TOOL_CALL_INTERRUPTED: &str = "Tool call interrupted";

/// `read_tool` 创建只读 workspace 内容读取工具。
pub fn read_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    read_tool_with_access(
        root,
        local_workspace_access(),
        WorkspaceReadState::default(),
    )
}

pub(crate) fn read_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
) -> impl Tool + 'static {
    ReadTool {
        root: root.as_ref().to_path_buf(),
        access,
        read_state,
    }
}

#[derive(Clone)]
struct ReadTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
}

impl std::fmt::Debug for ReadTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for ReadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(READ_TOOL_NAME)
            .with_label("Read")
            .with_kind(ToolKind::Read)
            .with_description(
                "Read a UTF-8 text file inside the current workspace. Use offset and limit to read large text files in chunks; text output includes 1-based line numbers.",
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
                        "description": "1-based line number to start reading from for text files"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": READ_MAX_LINE_COUNT,
                        "description": "Maximum number of text lines to return"
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_prompt_guidelines(
                "Prefer read over cat for reading files. Use offset and limit for large files.",
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
        let call_id = call.call_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            match task::spawn_blocking(move || {
                execute_read(root, access, read_state, call, cancellation)
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
struct ReadArguments {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug)]
struct ReadTextOutcome {
    output: String,
    start_line: usize,
    end_line: usize,
    total_lines: usize,
    next_offset: Option<usize>,
    fingerprint: TextFingerprint,
    is_complete: bool,
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("read task failed: {error}"))
}

fn execute_read(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    call: ToolCall,
    cancellation: CancellationToken,
) -> ToolResult {
    let arguments = match serde_json::from_value::<ReadArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(call.call_id, format!("read arguments are invalid: {error}"));
        }
    };

    let path = match resolve_workspace_path(access.as_ref(), &root, &arguments.path) {
        Ok(path) => path,
        Err(message) => return ToolResult::error(call.call_id, message),
    };

    let metadata = match access.metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                format!("stat failed for '{}': {error}", arguments.path),
            );
        }
    };
    if metadata.is_dir {
        return ToolResult::error(
            call.call_id,
            format!("'{}' is a directory, use list_dir instead", arguments.path),
        );
    }
    if !metadata.is_file {
        return ToolResult::error(
            call.call_id,
            format!("'{}' is not a regular file", arguments.path),
        );
    }
    if let Some(mime_type) = explicit_attachment_mime_type_for_path(&path) {
        return ToolResult::error(
            call.call_id,
            format!(
                "read failed for '{}': {mime_type} files must be attached explicitly in the user prompt instead of using read",
                path.display()
            ),
        );
    }

    match read_text_file_lines(
        access.as_ref(),
        &path,
        arguments.offset,
        arguments.limit,
        &cancellation,
    ) {
        Ok(outcome) => {
            read_state.record(
                path.clone(),
                WorkspaceFileSnapshot {
                    fingerprint: outcome.fingerprint,
                    modified_at: metadata.modified_at,
                    is_complete: outcome.is_complete,
                },
            );
            let mut result = ToolResult::success(call.call_id, outcome.output);
            result.details = Some(json!({
                "path": arguments.path,
                "kind": "text",
                "start_line": outcome.start_line,
                "end_line": outcome.end_line,
                "total_lines": outcome.total_lines,
                "next_offset": outcome.next_offset,
            }));
            result
        }
        Err(message) => ToolResult::error(call.call_id, message),
    }
}

fn read_text_file_lines(
    access: &dyn WorkspaceAccess,
    path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
    cancellation: &CancellationToken,
) -> Result<ReadTextOutcome, String> {
    let start_line = offset.unwrap_or(1).max(1);
    let line_limit = limit
        .unwrap_or(READ_DEFAULT_LINE_COUNT)
        .clamp(1, READ_MAX_LINE_COUNT);
    let end_line = start_line.saturating_add(line_limit.saturating_sub(1));

    let reader = access
        .open_reader(path)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    let mut reader = BufReader::new(reader);
    let mut raw_line = Vec::new();
    let mut selected = Vec::new();
    let mut total_lines = 0usize;
    let mut output_bytes = 0usize;
    let mut truncated_by_bytes = false;
    let mut truncated_by_line = false;
    let mut fingerprint = TextFingerprintBuilder::default();

    loop {
        if cancellation.is_cancelled() {
            return Err(TOOL_CALL_INTERRUPTED.to_string());
        }
        raw_line.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut raw_line)
            .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        fingerprint.update(&raw_line);

        total_lines += 1;

        while raw_line
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            raw_line.pop();
        }
        let line = std::str::from_utf8(&raw_line).map_err(|_| {
            format!(
                "read failed for '{}': file is not valid UTF-8 text",
                path.display()
            )
        })?;
        if contains_disallowed_text_control_chars(line) {
            return Err(format!(
                "read failed for '{}': file is not valid UTF-8 text",
                path.display()
            ));
        }
        if total_lines < start_line || total_lines > end_line || truncated_by_bytes {
            continue;
        }
        let (rendered_line, is_line_truncated) = truncate_line(line, READ_MAX_LINE_CHARS);
        truncated_by_line |= is_line_truncated;
        let rendered = format!("{total_lines}\t{rendered_line}");
        let rendered_bytes = rendered.len() + usize::from(!selected.is_empty());
        if output_bytes + rendered_bytes > READ_MAX_OUTPUT_BYTES {
            truncated_by_bytes = true;
            continue;
        }

        output_bytes += rendered_bytes;
        selected.push(rendered);
    }

    if total_lines == 0 {
        return Ok(ReadTextOutcome {
            output: String::new(),
            start_line,
            end_line: 0,
            total_lines: 0,
            next_offset: None,
            fingerprint: fingerprint.finish(),
            is_complete: offset.is_none() && limit.is_none(),
        });
    }

    if start_line > total_lines {
        return Err(format!(
            "read failed for '{}': offset {start_line} is beyond end of file ({total_lines} lines total)",
            path.display()
        ));
    }

    let rendered_start = start_line;
    let rendered_end = start_line + selected.len().saturating_sub(1);
    let next_offset = (total_lines > rendered_end).then_some(rendered_end + 1);
    let is_complete =
        offset.is_none() && limit.is_none() && next_offset.is_none() && !truncated_by_line;
    let mut output = selected.join("\n");
    if let Some(next_offset) = next_offset {
        if truncated_by_bytes {
            output.push_str(&format!(
                "\n\n[Showing lines {rendered_start}-{rendered_end} of {total_lines} (64KB output limit). Use offset={next_offset} to continue.]"
            ));
        } else {
            output.push_str(&format!(
                "\n\n[Showing lines {rendered_start}-{rendered_end} of {total_lines}. Use offset={next_offset} to continue.]"
            ));
        }
    }

    Ok(ReadTextOutcome {
        output,
        start_line: rendered_start,
        end_line: rendered_end,
        total_lines,
        next_offset,
        fingerprint: fingerprint.finish(),
        is_complete,
    })
}

fn explicit_attachment_mime_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "heic" => Some("image/heic"),
        "heif" => Some("image/heif"),
        "svg" => Some("image/svg+xml"),
        "wav" => Some("audio/wav"),
        "mp3" => Some("audio/mp3"),
        "ogg" => Some("audio/ogg"),
        "flac" => Some("audio/flac"),
        "m4a" => Some("audio/m4a"),
        "aac" => Some("audio/aac"),
        "pdf" => Some("application/pdf"),
        _ => None,
    }
}

fn contains_disallowed_text_control_chars(text: &str) -> bool {
    text.chars()
        .any(|ch| ch != '\n' && ch != '\r' && ch != '\t' && ch.is_control())
}

fn truncate_line(line: &str, max_chars: usize) -> (String, bool) {
    let mut chars = line.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        (format!("{truncated}..."), true)
    } else {
        (truncated, false)
    }
}
