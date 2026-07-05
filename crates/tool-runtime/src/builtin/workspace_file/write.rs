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
    error::WorkspaceFileError,
    file_state::WorkspaceReadState,
    mutation::{
        WorkspaceMutationQueue, bounded_permission_preview, bounded_tool_result_details,
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
        WorkspaceMutationQueue::default(),
    )
}

pub(crate) fn write_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    mutation_queue: WorkspaceMutationQueue,
) -> impl Tool + 'static {
    WriteTool {
        root: root.as_ref().to_path_buf(),
        access,
        read_state,
        mutation_queue,
    }
}

#[derive(Clone)]
struct WriteTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
    read_state: WorkspaceReadState,
    mutation_queue: WorkspaceMutationQueue,
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
            .with_prompt_guidelines("Use write for new files or full rewrites.")
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
                execute_write(
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
                execute_write(
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
    mutation_queue: WorkspaceMutationQueue,
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
        Err(error) => return ToolResult::error(call.call_id, error.to_string()),
    };

    match write_text_file(WriteTextFileOptions {
        access: access.as_ref(),
        read_state: &read_state,
        path: &path,
        requested_path: &arguments.path,
        content: &arguments.content,
        permission_snapshot: permission_snapshot.as_ref(),
        cancellation: &cancellation,
        mutation_queue: &mutation_queue,
    }) {
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
        Err(error) => ToolResult::error(call.call_id, error.to_string()),
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

struct WriteTextFileOptions<'a> {
    access: &'a dyn super::workspace_access::WorkspaceAccess,
    read_state: &'a WorkspaceReadState,
    path: &'a Path,
    requested_path: &'a str,
    content: &'a str,
    permission_snapshot: Option<&'a ToolPermissionFileSnapshot>,
    cancellation: &'a CancellationToken,
    mutation_queue: &'a WorkspaceMutationQueue,
}

fn write_text_file(options: WriteTextFileOptions<'_>) -> Result<WriteOutcome, WorkspaceFileError> {
    let WriteTextFileOptions {
        access,
        read_state,
        path,
        requested_path,
        content,
        permission_snapshot,
        cancellation,
        mutation_queue,
    } = options;

    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }

    let _mutation_guard = mutation_queue.lock_path(path);
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
        return Err(WorkspaceFileError::Interrupted);
    }
    if let Some(parent) = path.parent() {
        access.create_dir_all(parent).map_err(|source| {
            WorkspaceFileError::CreateParentDirectory {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    access
        .write_text_file(path, content)
        .map_err(|source| WorkspaceFileError::Write {
            path: path.to_path_buf(),
            source,
        })?;
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

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Cursor, Read},
        path::{Path, PathBuf},
        sync::{
            Arc, Barrier, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use tokio_util::sync::CancellationToken;

    use super::super::{
        edit::edit_tool_with_access,
        file_state::{TextFingerprint, WorkspaceFileSnapshot, WorkspaceReadState},
        mutation::WorkspaceMutationQueue,
        workspace_access::{
            SharedWorkspaceAccess, WorkspaceAccess, WorkspaceDirectoryEntry, WorkspaceMetadata,
        },
    };
    use super::write_tool_with_access;
    use crate::{ToolCall, ToolExecutor, ToolExecutorRegistry};

    #[test]
    fn write_tool_serializes_concurrent_mutations_to_same_file() {
        let root = PathBuf::from("/workspace");
        let access = Arc::new(ConcurrentWriteAccess::new(root.clone()));
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(write_tool_with_access(
            &root,
            access.clone() as SharedWorkspaceAccess,
            WorkspaceReadState::default(),
            WorkspaceMutationQueue::default(),
        ));
        let start = Arc::new(Barrier::new(2));
        let first_registry = registry.clone();
        let second_registry = registry;
        let first_start = start.clone();
        let second_start = start;

        let first = thread::spawn(move || {
            first_start.wait();
            run_tool(
                first_registry,
                ToolCall::new(
                    "write-1",
                    "write",
                    serde_json::json!({
                        "path": "notes.txt",
                        "content": "first\n"
                    }),
                ),
            )
        });
        let second = thread::spawn(move || {
            second_start.wait();
            run_tool(
                second_registry,
                ToolCall::new(
                    "write-2",
                    "write",
                    serde_json::json!({
                        "path": "notes.txt",
                        "content": "second\n"
                    }),
                ),
            )
        });

        let _ = first.join().expect("first write thread should finish");
        let _ = second.join().expect("second write thread should finish");

        assert_eq!(
            access.max_active_writes(),
            1,
            "same-file mutations must not enter the backend write path concurrently"
        );
    }

    #[test]
    fn write_and_edit_tools_share_same_file_mutation_queue() {
        let root = PathBuf::from("/workspace");
        let original = "first\nsecond\n";
        let access = Arc::new(ConcurrentWriteAccess::new_with_content(
            root.clone(),
            original,
        ));
        let read_state = WorkspaceReadState::default();
        read_state.record(
            access.file_path(),
            WorkspaceFileSnapshot {
                fingerprint: TextFingerprint::from_text(original),
                modified_at: None,
                is_complete: true,
            },
        );
        let mutation_queue = WorkspaceMutationQueue::default();
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(write_tool_with_access(
            &root,
            access.clone() as SharedWorkspaceAccess,
            read_state.clone(),
            mutation_queue.clone(),
        ));
        registry.insert(edit_tool_with_access(
            &root,
            access.clone() as SharedWorkspaceAccess,
            read_state,
            mutation_queue,
        ));
        let start = Arc::new(Barrier::new(2));
        let first_registry = registry.clone();
        let second_registry = registry;
        let first_start = start.clone();
        let second_start = start;

        let first = thread::spawn(move || {
            first_start.wait();
            run_tool(
                first_registry,
                ToolCall::new(
                    "write-1",
                    "write",
                    serde_json::json!({
                        "path": "notes.txt",
                        "content": "written\nsecond\n"
                    }),
                ),
            )
        });
        let second = thread::spawn(move || {
            second_start.wait();
            run_tool(
                second_registry,
                ToolCall::new(
                    "edit-1",
                    "edit",
                    serde_json::json!({
                        "path": "notes.txt",
                        "edits": [
                            { "old_string": "second", "new_string": "edited" }
                        ]
                    }),
                ),
            )
        });

        let first_result = first.join().expect("write thread should finish");
        let second_result = second.join().expect("edit thread should finish");

        assert!(
            !first_result.is_error(),
            "write should succeed: {first_result:?}"
        );
        assert!(
            !second_result.is_error(),
            "edit should succeed: {second_result:?}"
        );
        assert_eq!(
            access.max_active_writes(),
            1,
            "write and edit must share the same same-file mutation queue"
        );
    }

    fn run_tool(registry: ToolExecutorRegistry, call: ToolCall) -> crate::ToolResult {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime should build");
        runtime
            .block_on(async move { registry.execute_tool(call, &CancellationToken::new()).await })
    }

    struct ConcurrentWriteAccess {
        root: PathBuf,
        content: Mutex<Option<String>>,
        active_writes: AtomicUsize,
        max_active_writes: AtomicUsize,
    }

    impl ConcurrentWriteAccess {
        fn new(root: PathBuf) -> Self {
            Self::new_with_optional_content(root, None)
        }

        fn new_with_content(root: PathBuf, content: &str) -> Self {
            Self::new_with_optional_content(root, Some(content.to_string()))
        }

        fn new_with_optional_content(root: PathBuf, content: Option<String>) -> Self {
            Self {
                root,
                content: Mutex::new(content),
                active_writes: AtomicUsize::new(0),
                max_active_writes: AtomicUsize::new(0),
            }
        }

        fn file_path(&self) -> PathBuf {
            self.root.join("notes.txt")
        }

        fn max_active_writes(&self) -> usize {
            self.max_active_writes.load(Ordering::SeqCst)
        }
    }

    impl WorkspaceAccess for ConcurrentWriteAccess {
        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
            if path == self.root {
                return Ok(self.root.clone());
            }
            if path == self.file_path() && self.content.lock().unwrap().is_some() {
                return Ok(path.to_path_buf());
            }
            Err(io::Error::new(io::ErrorKind::NotFound, "missing path"))
        }

        fn metadata(&self, path: &Path) -> io::Result<WorkspaceMetadata> {
            if path == self.root {
                return Ok(WorkspaceMetadata {
                    is_dir: true,
                    is_file: false,
                    len: 0,
                    modified_at: None,
                });
            }
            if path == self.file_path()
                && let Some(content) = self.content.lock().unwrap().as_ref()
            {
                return Ok(WorkspaceMetadata {
                    is_dir: false,
                    is_file: true,
                    len: content.len() as u64,
                    modified_at: None,
                });
            }
            Err(io::Error::new(io::ErrorKind::NotFound, "missing path"))
        }

        fn open_reader(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
            if path == self.file_path()
                && let Some(content) = self.content.lock().unwrap().clone()
            {
                return Ok(Box::new(Cursor::new(content.into_bytes())));
            }
            Err(io::Error::new(io::ErrorKind::NotFound, "missing path"))
        }

        fn read_dir(&self, _path: &Path) -> io::Result<Vec<WorkspaceDirectoryEntry>> {
            Ok(Vec::new())
        }

        fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }

        fn write_text_file(&self, path: &Path, content: &str) -> io::Result<()> {
            if path != self.file_path() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "missing path"));
            }
            let active = self.active_writes.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_writes.fetch_max(active, Ordering::SeqCst);
            if active == 1 {
                thread::sleep(Duration::from_millis(75));
            }
            *self.content.lock().unwrap() = Some(content.to_string());
            self.active_writes.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }
    }
}
