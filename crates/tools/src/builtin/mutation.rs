use std::{io, path::Path};

use tokio_util::sync::CancellationToken;

use super::{
    file_state::{
        TextFingerprint, WorkspaceFileSnapshot, WorkspaceReadState, text_fingerprint_from_reader,
    },
    workspace_access::{WorkspaceAccess, WorkspaceMetadata},
};

pub(crate) const TOOL_CALL_INTERRUPTED: &str = "Tool call interrupted";
pub(crate) const FILE_NOT_READ_MESSAGE: &str =
    "File has not been read yet. Read it first before writing to it.";
pub(crate) const FILE_CHANGED_MESSAGE: &str = "File has been modified since read, either by the user or by a linter. Read it again before attempting to write it.";

pub(crate) fn existing_file_metadata(
    access: &dyn WorkspaceAccess,
    path: &Path,
    requested_path: &str,
    tool_name: &str,
) -> Result<Option<WorkspaceMetadata>, String> {
    match access.metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir {
                return Err(format!(
                    "'{requested_path}' is a directory, cannot {tool_name} a directory"
                ));
            }
            if !metadata.is_file {
                return Err(format!("'{requested_path}' is not a regular file"));
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("stat failed for '{requested_path}': {error}")),
    }
}

pub(crate) fn ensure_existing_file_was_read(
    access: &dyn WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    _metadata: &WorkspaceMetadata,
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }

    let snapshot = read_state.snapshot(path).ok_or(FILE_NOT_READ_MESSAGE)?;
    if !snapshot.is_complete {
        return Err(FILE_NOT_READ_MESSAGE.to_string());
    }

    let fingerprint = current_file_fingerprint(access, path, cancellation)?;
    if fingerprint != snapshot.fingerprint {
        return Err(FILE_CHANGED_MESSAGE.to_string());
    }

    Ok(())
}

pub(crate) fn record_written_text_snapshot(
    access: &dyn WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    content: &str,
) {
    if let Ok(metadata) = access.metadata(path) {
        read_state.record(
            path.to_path_buf(),
            WorkspaceFileSnapshot {
                fingerprint: TextFingerprint::from_text(content),
                modified_at: metadata.modified_at,
                is_complete: true,
            },
        );
    }
}

fn current_file_fingerprint(
    access: &dyn WorkspaceAccess,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<TextFingerprint, String> {
    if cancellation.is_cancelled() {
        return Err(TOOL_CALL_INTERRUPTED.to_string());
    }
    let mut reader = access
        .open_reader(path)
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))?;
    text_fingerprint_from_reader(reader.as_mut())
        .map_err(|error| format!("read failed for '{}': {error}", path.display()))
}
