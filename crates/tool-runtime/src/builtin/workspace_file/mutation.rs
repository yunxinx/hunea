use std::{
    collections::HashSet,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
};

use tokio_util::sync::CancellationToken;

use super::{
    error::WorkspaceFileError,
    file_state::{
        TextFingerprint, WorkspaceFileSnapshot, WorkspaceReadState, text_fingerprint_from_reader,
    },
    workspace_access::{WorkspaceAccess, WorkspaceMetadata},
};
use crate::{ToolPermissionFileSnapshot, ToolPermissionPreview};

pub(crate) const FILE_PERMISSION_PREVIEW_MAX_TOTAL_BYTES: usize = 256 * 1024;
pub(crate) const FILE_PERMISSION_PREVIEW_MAX_LINES: usize = 6_000;

/// `WorkspaceMutationQueue` 串行化同一 workspace 路径上的写入型工具调用。
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceMutationQueue {
    state: Arc<WorkspaceMutationQueueState>,
}

#[derive(Debug, Default)]
struct WorkspaceMutationQueueState {
    active_paths: Mutex<HashSet<PathBuf>>,
    path_released: Condvar,
}

pub(crate) struct WorkspaceMutationGuard<'a> {
    queue: &'a WorkspaceMutationQueue,
    path: PathBuf,
}

impl WorkspaceMutationQueue {
    pub(crate) fn lock_path(&self, path: &Path) -> WorkspaceMutationGuard<'_> {
        let path = path.to_path_buf();
        let mut active_paths = self
            .state
            .active_paths
            .lock()
            .expect("workspace mutation queue lock should not be poisoned");

        while active_paths.contains(&path) {
            active_paths = self
                .state
                .path_released
                .wait(active_paths)
                .expect("workspace mutation queue lock should not be poisoned");
        }

        active_paths.insert(path.clone());
        WorkspaceMutationGuard { queue: self, path }
    }
}

impl Drop for WorkspaceMutationGuard<'_> {
    fn drop(&mut self) {
        let mut active_paths = self
            .queue
            .state
            .active_paths
            .lock()
            .expect("workspace mutation queue lock should not be poisoned");
        active_paths.remove(&self.path);
        self.queue.state.path_released.notify_all();
    }
}

pub(crate) fn existing_file_metadata(
    access: &dyn WorkspaceAccess,
    path: &Path,
    requested_path: &str,
    tool_name: &'static str,
) -> Result<Option<WorkspaceMetadata>, WorkspaceFileError> {
    match access.metadata(path) {
        Ok(metadata) => {
            if metadata.is_dir {
                return Err(WorkspaceFileError::DirectoryMutation {
                    path: requested_path.to_string(),
                    operation: tool_name,
                });
            }
            if !metadata.is_file {
                return Err(WorkspaceFileError::NotRegularFile {
                    path: requested_path.to_string(),
                });
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(WorkspaceFileError::Metadata {
            path: requested_path.to_string(),
            source,
        }),
    }
}

pub(crate) fn ensure_existing_file_was_read(
    access: &dyn WorkspaceAccess,
    read_state: &WorkspaceReadState,
    permission_snapshot: Option<&ToolPermissionFileSnapshot>,
    path: &Path,
    _metadata: &WorkspaceMetadata,
    cancellation: &CancellationToken,
) -> Result<(), WorkspaceFileError> {
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }

    let snapshot = permission_snapshot
        .map(workspace_snapshot_from_permission_snapshot)
        .or_else(|| read_state.snapshot(path))
        .ok_or(WorkspaceFileError::FileNotRead)?;
    if !snapshot.is_complete {
        return Err(WorkspaceFileError::FileNotRead);
    }

    let fingerprint = current_file_fingerprint(access, path, cancellation)?;
    if fingerprint != snapshot.fingerprint {
        return Err(WorkspaceFileError::FileChanged);
    }

    Ok(())
}

pub(crate) fn record_written_text_snapshot(
    access: &dyn WorkspaceAccess,
    read_state: &WorkspaceReadState,
    path: &Path,
    content: &str,
) {
    record_complete_text_snapshot(access, read_state, path, content);
}

pub(crate) fn record_complete_text_snapshot(
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

pub(crate) fn permission_file_snapshot(
    metadata: &WorkspaceMetadata,
    content: &str,
) -> ToolPermissionFileSnapshot {
    let fingerprint = TextFingerprint::from_text(content);
    ToolPermissionFileSnapshot {
        content_hash: fingerprint.hash(),
        byte_len: fingerprint.byte_len(),
        modified_at: metadata.modified_at,
    }
}

pub(crate) fn bounded_permission_preview(
    path: String,
    old_text: Option<String>,
    new_text: String,
    snapshot: Option<ToolPermissionFileSnapshot>,
) -> ToolPermissionPreview {
    let (old_text, new_text, is_truncated) = bounded_permission_preview_text(old_text, new_text);
    ToolPermissionPreview {
        path,
        old_text,
        new_text,
        is_truncated,
        snapshot,
    }
}

pub(crate) fn read_existing_text_file(
    access: &dyn WorkspaceAccess,
    path: &Path,
) -> Result<String, WorkspaceFileError> {
    let mut reader = access
        .open_reader(path)
        .map_err(|source| WorkspaceFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|source| WorkspaceFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    String::from_utf8(bytes).map_err(|_| WorkspaceFileError::ReadRejected {
        path: path.to_path_buf(),
        detail: "file is not valid UTF-8 text".to_string(),
    })
}

pub(crate) fn bounded_tool_result_details(
    path: String,
    old_text: Option<String>,
    new_text: String,
) -> serde_json::Value {
    let (old_text, new_text, is_truncated) = bounded_permission_preview_text(old_text, new_text);
    let mut details = serde_json::Map::new();
    details.insert("path".to_string(), serde_json::json!(path));
    if let Some(old_text) = old_text {
        details.insert("old_text".to_string(), serde_json::json!(old_text));
    }
    details.insert("new_text".to_string(), serde_json::json!(new_text));
    details.insert(
        "preview_truncated".to_string(),
        serde_json::json!(is_truncated),
    );
    serde_json::Value::Object(details)
}

fn workspace_snapshot_from_permission_snapshot(
    snapshot: &ToolPermissionFileSnapshot,
) -> WorkspaceFileSnapshot {
    WorkspaceFileSnapshot {
        fingerprint: TextFingerprint::from_parts(snapshot.content_hash, snapshot.byte_len),
        modified_at: snapshot.modified_at,
        is_complete: true,
    }
}

fn bounded_permission_preview_text(
    old_text: Option<String>,
    new_text: String,
) -> (Option<String>, String, bool) {
    let total_bytes = old_text.as_deref().map(str::len).unwrap_or(0) + new_text.len();
    let total_lines = old_text.as_deref().map(line_count).unwrap_or(0) + line_count(&new_text);
    if total_bytes <= FILE_PERMISSION_PREVIEW_MAX_TOTAL_BYTES
        && total_lines <= FILE_PERMISSION_PREVIEW_MAX_LINES
    {
        return (old_text, new_text, false);
    }

    let old_byte_len = old_text.as_deref().map(str::len).unwrap_or(0);
    let old_line_count = old_text.as_deref().map(line_count).unwrap_or(0);
    let (old_byte_budget, new_byte_budget) = split_budget(
        old_byte_len,
        new_text.len(),
        FILE_PERMISSION_PREVIEW_MAX_TOTAL_BYTES,
    );
    let (old_line_budget, new_line_budget) = split_budget(
        old_line_count,
        line_count(&new_text),
        FILE_PERMISSION_PREVIEW_MAX_LINES,
    );

    let old_text =
        old_text.map(|text| truncate_preview_text(&text, old_byte_budget, old_line_budget));
    let new_text = truncate_preview_text(&new_text, new_byte_budget, new_line_budget);
    (old_text, new_text, true)
}

fn split_budget(left_len: usize, right_len: usize, total_budget: usize) -> (usize, usize) {
    if left_len == 0 {
        return (0, total_budget);
    }
    let half_budget = total_budget / 2;
    if left_len <= half_budget {
        return (left_len, total_budget.saturating_sub(left_len));
    }
    if right_len <= half_budget {
        return (total_budget.saturating_sub(right_len), right_len);
    }
    (half_budget, total_budget.saturating_sub(half_budget))
}

fn truncate_preview_text(text: &str, max_bytes: usize, max_lines: usize) -> String {
    if max_bytes == 0 || max_lines == 0 || text.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    let mut lines = 0usize;
    for segment in text.split_inclusive('\n') {
        if lines >= max_lines || output.len() >= max_bytes {
            break;
        }

        let remaining_bytes = max_bytes.saturating_sub(output.len());
        if segment.len() <= remaining_bytes {
            output.push_str(segment);
        } else {
            output.push_str(utf8_prefix(segment, remaining_bytes));
            break;
        }
        lines = lines.saturating_add(1);
    }

    output
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn current_file_fingerprint(
    access: &dyn WorkspaceAccess,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<TextFingerprint, WorkspaceFileError> {
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    let mut reader = access
        .open_reader(path)
        .map_err(|source| WorkspaceFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    text_fingerprint_from_reader(reader.as_mut()).map_err(|source| WorkspaceFileError::Read {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Read},
        path::{Path, PathBuf},
    };

    use super::{
        super::{
            error::WorkspaceFileError,
            workspace_access::{WorkspaceAccess, WorkspaceDirectoryEntry, WorkspaceMetadata},
        },
        read_existing_text_file,
    };

    struct FailingReadAccess;

    impl WorkspaceAccess for FailingReadAccess {
        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
            Ok(path.to_path_buf())
        }

        fn metadata(&self, _path: &Path) -> io::Result<WorkspaceMetadata> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "metadata is not used in this test",
            ))
        }

        fn open_reader(&self, _path: &Path) -> io::Result<Box<dyn Read + Send>> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "read is denied",
            ))
        }

        fn read_dir(&self, _path: &Path) -> io::Result<Vec<WorkspaceDirectoryEntry>> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "read_dir is not used in this test",
            ))
        }
    }

    #[test]
    fn read_existing_text_file_preserves_read_source() {
        let error = read_existing_text_file(&FailingReadAccess, Path::new("/workspace/secret.txt"))
            .expect_err("read failure should be returned");

        let WorkspaceFileError::Read { path, source } = error else {
            panic!("read failure should retain the filesystem source error");
        };
        assert_eq!(path, PathBuf::from("/workspace/secret.txt"));
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
    }
}
