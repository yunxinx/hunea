use std::{
    io::Read,
    path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolImageDetail, ToolKind,
    ToolPermissionPolicy, ToolResult, ToolResultContent,
};

use super::{
    error::WorkspaceFileError,
    workspace::resolve_workspace_path,
    workspace_access::{SharedWorkspaceAccess, local_workspace_access},
};

const VIEW_IMAGE_TOOL_NAME: &str = "view_image";
const VIEW_IMAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// `view_image_tool` 创建本地 workspace 图片查看工具。
pub fn view_image_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    view_image_tool_with_access(root, local_workspace_access())
}

pub(crate) fn view_image_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
) -> impl Tool + 'static {
    ViewImageTool {
        root: root.as_ref().to_path_buf(),
        access,
    }
}

#[derive(Clone)]
struct ViewImageTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
}

impl std::fmt::Debug for ViewImageTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ViewImageTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for ViewImageTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(VIEW_IMAGE_TOOL_NAME)
            .with_label("View Image")
            .with_kind(ToolKind::Read)
            .with_description(
                "Attach a local image file from the current workspace so the model can inspect it visually.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute image path"
                    },
                    "detail": {
                        "type": "string",
                        "enum": ["high", "original"],
                        "description": "Image detail level. Defaults to high; original preserves the file bytes."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_prompt_guidelines(
                "Use view_image instead of read when a local PNG, JPEG, GIF, or WebP file needs visual inspection.",
            )
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let root = self.root.clone();
        let access = self.access.clone();
        let call_id = call.call_id.clone();
        let cancellation = cancellation.clone();
        Box::pin(async move {
            match task::spawn_blocking(move || execute_view_image(root, access, call, cancellation))
                .await
            {
                Ok(result) => result,
                Err(error) => join_error_result(call_id, error),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct ViewImageArguments {
    path: String,
    detail: Option<ViewImageDetail>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ViewImageDetail {
    High,
    Original,
}

impl From<ViewImageDetail> for ToolImageDetail {
    fn from(detail: ViewImageDetail) -> Self {
        match detail {
            ViewImageDetail::High => Self::High,
            ViewImageDetail::Original => Self::Original,
        }
    }
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("view_image task failed: {error}"))
}

fn execute_view_image(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    call: ToolCall,
    cancellation: CancellationToken,
) -> ToolResult {
    let arguments = match serde_json::from_value::<ViewImageArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(source) => {
            return ToolResult::error(
                call.call_id,
                WorkspaceFileError::InvalidArguments {
                    tool: "view_image",
                    source,
                }
                .to_string(),
            );
        }
    };

    let detail = arguments.detail.unwrap_or(ViewImageDetail::High).into();

    if cancellation.is_cancelled() {
        return ToolResult::error(call.call_id, WorkspaceFileError::Interrupted.to_string());
    }

    let path = match resolve_workspace_path(access.as_ref(), &root, &arguments.path) {
        Ok(path) => path,
        Err(error) => return ToolResult::error(call.call_id, error.to_string()),
    };
    let metadata = match access.metadata(&path) {
        Ok(metadata) => metadata,
        Err(source) => {
            return ToolResult::error(
                call.call_id,
                WorkspaceFileError::Metadata {
                    path: arguments.path,
                    source,
                }
                .to_string(),
            );
        }
    };
    if !metadata.is_file {
        return ToolResult::error(
            call.call_id,
            WorkspaceFileError::ImageNotFile {
                path: arguments.path,
            }
            .to_string(),
        );
    }
    if metadata.len > VIEW_IMAGE_MAX_BYTES {
        return ToolResult::error(
            call.call_id,
            WorkspaceFileError::ImageTooLarge {
                path: arguments.path,
                bytes: metadata.len,
                limit: VIEW_IMAGE_MAX_BYTES,
            }
            .to_string(),
        );
    }

    let mime_type = match supported_image_mime_type_for_path(&path) {
        Some(mime_type) => mime_type,
        None => {
            return ToolResult::error(
                call.call_id,
                WorkspaceFileError::UnsupportedImageType {
                    path: arguments.path,
                }
                .to_string(),
            );
        }
    };

    let bytes = match read_file_bytes(access.as_ref(), &path, &cancellation) {
        Ok(bytes) => bytes,
        Err(error) => return ToolResult::error(call.call_id, error.to_string()),
    };
    if !image_signature_matches(mime_type, &bytes) {
        return ToolResult::error(
            call.call_id,
            WorkspaceFileError::ImageSignatureMismatch {
                path: arguments.path,
                mime_type,
            }
            .to_string(),
        );
    }

    ToolResult::success_content(
        call.call_id,
        vec![ToolResultContent::Image {
            data_base64: BASE64_STANDARD.encode(bytes),
            mime_type: mime_type.to_string(),
            uri: Some(arguments.path),
            detail: Some(detail),
        }],
    )
}

fn read_file_bytes(
    access: &dyn super::workspace_access::WorkspaceAccess,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<Vec<u8>, WorkspaceFileError> {
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    let mut reader = access
        .open_reader(path)
        .map_err(|source| WorkspaceFileError::ReadImage {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|source| WorkspaceFileError::ReadImage {
            path: path.to_path_buf(),
            source,
        })?;
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    Ok(bytes)
}

fn supported_image_mime_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn image_signature_matches(mime_type: &str, bytes: &[u8]) -> bool {
    match mime_type {
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}
