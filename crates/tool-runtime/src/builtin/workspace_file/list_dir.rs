use std::{
    io::{self, Read},
    path::{Path, PathBuf},
};

use ignore::{
    Match,
    gitignore::{Gitignore, GitignoreBuilder},
};
use serde::Deserialize;
use serde_json::json;
use tokio::{task, task::JoinError};
use tokio_util::sync::CancellationToken;

use crate::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolKind, ToolPermissionPolicy, ToolResult,
};

use super::{
    error::WorkspaceFileError,
    workspace::resolve_workspace_path,
    workspace_access::{SharedWorkspaceAccess, local_workspace_access},
};

const LIST_DIR_TOOL_NAME: &str = "list_dir";
const LIST_DIR_DEFAULT_ENTRY_LIMIT: usize = 500;
const LIST_DIR_MAX_ENTRY_LIMIT: usize = 2_000;

/// `list_dir_tool` 创建只读 workspace 目录列举工具。
pub fn list_dir_tool(root: impl AsRef<Path>) -> impl Tool + 'static {
    list_dir_tool_with_access(root, local_workspace_access())
}

pub(crate) fn list_dir_tool_with_access(
    root: impl AsRef<Path>,
    access: SharedWorkspaceAccess,
) -> impl Tool + 'static {
    ListDirTool {
        root: root.as_ref().to_path_buf(),
        access,
    }
}

struct ListDirTool {
    root: PathBuf,
    access: SharedWorkspaceAccess,
}

impl std::fmt::Debug for ListDirTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ListDirTool")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Tool for ListDirTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(LIST_DIR_TOOL_NAME)
            .with_label("List Directory")
            .with_kind(ToolKind::Search)
            .with_description(
                "List immediate entries of a directory inside the current workspace. Entries are sorted alphabetically, include dotfiles unless gitignored, and directories end with '/'.",
            )
            .with_input_schema(json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or workspace-contained absolute directory path; defaults to the workspace root"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": LIST_DIR_MAX_ENTRY_LIMIT,
                        "description": "Maximum number of entries to return"
                    }
                },
                "additionalProperties": false
            }))
            .with_permission_policy(ToolPermissionPolicy::Always)
            .with_prompt_guidelines("Prefer list_dir over shell ls.")
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
            match task::spawn_blocking(move || execute_list_dir(root, access, call, cancellation))
                .await
            {
                Ok(result) => result,
                Err(error) => join_error_result(call_id, error),
            }
        })
    }
}

#[derive(Debug, Deserialize)]
struct ListDirArguments {
    path: Option<String>,
    limit: Option<usize>,
}

fn join_error_result(call_id: String, error: JoinError) -> ToolResult {
    ToolResult::error(call_id, format!("list_dir task failed: {error}"))
}

fn execute_list_dir(
    root: PathBuf,
    access: SharedWorkspaceAccess,
    call: ToolCall,
    cancellation: CancellationToken,
) -> ToolResult {
    let arguments = match serde_json::from_value::<ListDirArguments>(call.arguments) {
        Ok(arguments) => arguments,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                WorkspaceFileError::InvalidArguments {
                    tool: "list_dir",
                    source: error,
                }
                .to_string(),
            );
        }
    };
    let requested_path = arguments.path.as_deref().unwrap_or(".");

    let path = match resolve_workspace_path(access.as_ref(), &root, requested_path) {
        Ok(path) => path,
        Err(error) => return ToolResult::error(call.call_id, error.to_string()),
    };

    let metadata = match access.metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return ToolResult::error(
                call.call_id,
                WorkspaceFileError::Metadata {
                    path: requested_path.to_string(),
                    source: error,
                }
                .to_string(),
            );
        }
    };
    if !metadata.is_dir {
        return ToolResult::error(
            call.call_id,
            WorkspaceFileError::NotDirectory {
                path: requested_path.to_string(),
                replacement: "read",
            }
            .to_string(),
        );
    }

    let limit = arguments
        .limit
        .unwrap_or(LIST_DIR_DEFAULT_ENTRY_LIMIT)
        .clamp(1, LIST_DIR_MAX_ENTRY_LIMIT);
    match list_directory_entries(&root, access.as_ref(), &path, limit, &cancellation) {
        Ok(content) => ToolResult::success(call.call_id, content),
        Err(error) => ToolResult::error(call.call_id, error.to_string()),
    }
}

fn list_directory_entries(
    root: &Path,
    access: &dyn super::workspace_access::WorkspaceAccess,
    path: &Path,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<String, WorkspaceFileError> {
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    let root = access
        .canonicalize(root)
        .map_err(|source| WorkspaceFileError::WorkspaceRoot {
            path: root.to_path_buf(),
            source,
        })?;
    let gitignore = gitignore_matcher(access, &root, path, cancellation)?;
    let mut entries = access
        .read_dir(path)
        .map_err(|source| WorkspaceFileError::ReadDirectory {
            path: path.to_path_buf(),
            source,
        })?
        .into_iter()
        .filter_map(|entry| {
            if is_gitignored(&gitignore, &entry.path, entry.is_dir) {
                return None;
            }
            let display_name = if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name
            };
            Some(display_name)
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.to_lowercase());

    if entries.is_empty() {
        return Ok("No entries found.".to_string());
    }

    let total_entries = entries.len();
    let mut content = entries
        .iter()
        .take(limit)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    if total_entries > limit {
        let next_limit = limit.saturating_mul(2).min(LIST_DIR_MAX_ENTRY_LIMIT);
        if next_limit > limit {
            content.push_str(&format!(
                "\n\n[Truncated: showing {limit} of {total_entries} entries. Use limit={next_limit} for more.]"
            ));
        } else {
            content.push_str(&format!(
                "\n\n[Truncated: showing {limit} of {total_entries} entries. Maximum limit is {LIST_DIR_MAX_ENTRY_LIMIT}.]"
            ));
        }
    }
    Ok(content)
}

fn gitignore_matcher(
    access: &dyn super::workspace_access::WorkspaceAccess,
    root: &Path,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<Gitignore, WorkspaceFileError> {
    let mut builder = GitignoreBuilder::new(root);
    for directory in gitignore_directories(root, path) {
        if cancellation.is_cancelled() {
            return Err(WorkspaceFileError::Interrupted);
        }
        let gitignore = directory.join(".gitignore");
        let Some(content) = read_optional_text_file(access, &gitignore, cancellation)? else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if cancellation.is_cancelled() {
                return Err(WorkspaceFileError::Interrupted);
            }
            let line = if index == 0 {
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            builder
                .add_line(Some(gitignore.clone()), line)
                .map_err(|source| WorkspaceFileError::Gitignore {
                    path: gitignore.clone(),
                    source,
                })?;
        }
    }
    builder
        .build()
        .map_err(|source| WorkspaceFileError::GitignoreMatcher {
            root: root.to_path_buf(),
            source,
        })
}

fn read_optional_text_file(
    access: &dyn super::workspace_access::WorkspaceAccess,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<Option<String>, WorkspaceFileError> {
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    let metadata = match access.metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(WorkspaceFileError::Metadata {
                path: path.display().to_string(),
                source: error,
            });
        }
    };
    if !metadata.is_file {
        return Ok(None);
    }

    let mut reader = access
        .open_reader(path)
        .map_err(|source| WorkspaceFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if cancellation.is_cancelled() {
        return Err(WorkspaceFileError::Interrupted);
    }
    let mut content = String::new();
    reader
        .read_to_string(&mut content)
        .map_err(|source| WorkspaceFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(Some(content))
}

fn gitignore_directories(root: &Path, path: &Path) -> Vec<PathBuf> {
    let mut directories = vec![root.to_path_buf()];
    let Ok(relative) = path.strip_prefix(root) else {
        return directories;
    };

    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        directories.push(current.clone());
    }
    directories
}

fn is_gitignored(gitignore: &Gitignore, path: &Path, is_dir: bool) -> bool {
    matches!(
        gitignore.matched_path_or_any_parents(path, is_dir),
        Match::Ignore(_)
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{self, Cursor, Read},
        path::{Path, PathBuf},
    };

    use super::super::workspace_access::{
        WorkspaceAccess, WorkspaceDirectoryEntry, WorkspaceMetadata,
    };
    use super::{super::error::WorkspaceFileError, list_directory_entries};
    use tokio_util::sync::CancellationToken;

    struct FakeWorkspaceAccess {
        canonical_paths: HashMap<PathBuf, PathBuf>,
        metadata_by_path: HashMap<PathBuf, WorkspaceMetadata>,
        file_contents: HashMap<PathBuf, Vec<u8>>,
        directories: HashMap<PathBuf, Vec<WorkspaceDirectoryEntry>>,
    }

    impl WorkspaceAccess for FakeWorkspaceAccess {
        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
            self.canonical_paths
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing canonical path"))
        }

        fn metadata(&self, path: &Path) -> io::Result<WorkspaceMetadata> {
            self.metadata_by_path
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing metadata"))
        }

        fn open_reader(&self, path: &Path) -> io::Result<Box<dyn Read + Send>> {
            self.file_contents
                .get(path)
                .cloned()
                .map(|content| Box::new(Cursor::new(content)) as Box<dyn Read + Send>)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing file content"))
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<WorkspaceDirectoryEntry>> {
            self.directories
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing directory"))
        }
    }

    #[test]
    fn list_directory_entries_reads_gitignore_through_workspace_access() {
        let access = FakeWorkspaceAccess {
            canonical_paths: HashMap::from([(
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            )]),
            metadata_by_path: HashMap::from([(
                PathBuf::from("/srv/workspace/src/.gitignore"),
                WorkspaceMetadata {
                    is_dir: false,
                    is_file: true,
                    len: 6,
                    modified_at: None,
                },
            )]),
            file_contents: HashMap::from([(
                PathBuf::from("/srv/workspace/src/.gitignore"),
                b"*.log\n".to_vec(),
            )]),
            directories: HashMap::from([(
                PathBuf::from("/srv/workspace/src"),
                vec![
                    WorkspaceDirectoryEntry {
                        path: PathBuf::from("/srv/workspace/src/keep.rs"),
                        name: "keep.rs".to_string(),
                        is_dir: false,
                    },
                    WorkspaceDirectoryEntry {
                        path: PathBuf::from("/srv/workspace/src/debug.log"),
                        name: "debug.log".to_string(),
                        is_dir: false,
                    },
                ],
            )]),
        };

        let content = list_directory_entries(
            Path::new("/workspace-link"),
            &access,
            Path::new("/srv/workspace/src"),
            10,
            &CancellationToken::new(),
        )
        .expect("directory listing should succeed");

        assert_eq!(content, "keep.rs");
    }

    #[test]
    fn list_directory_entries_reports_invalid_gitignore_patterns() {
        let access = FakeWorkspaceAccess {
            canonical_paths: HashMap::from([(
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            )]),
            metadata_by_path: HashMap::from([(
                PathBuf::from("/srv/workspace/src/.gitignore"),
                WorkspaceMetadata {
                    is_dir: false,
                    is_file: true,
                    len: 2,
                    modified_at: None,
                },
            )]),
            file_contents: HashMap::from([(
                PathBuf::from("/srv/workspace/src/.gitignore"),
                b"{foo\n".to_vec(),
            )]),
            directories: HashMap::from([(
                PathBuf::from("/srv/workspace/src"),
                vec![WorkspaceDirectoryEntry {
                    path: PathBuf::from("/srv/workspace/src/keep.rs"),
                    name: "keep.rs".to_string(),
                    is_dir: false,
                }],
            )]),
        };

        let error = list_directory_entries(
            Path::new("/workspace-link"),
            &access,
            Path::new("/srv/workspace/src"),
            10,
            &CancellationToken::new(),
        )
        .expect_err("invalid .gitignore should surface as an error");

        let WorkspaceFileError::Gitignore { path, source: _ } = error else {
            panic!("invalid .gitignore should retain its typed error source");
        };
        assert_eq!(path, PathBuf::from("/srv/workspace/src/.gitignore"));
    }

    #[test]
    fn list_directory_entries_applies_target_directory_gitignore_rules() {
        let access = FakeWorkspaceAccess {
            canonical_paths: HashMap::from([(
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            )]),
            metadata_by_path: HashMap::from([(
                PathBuf::from("/srv/workspace/src/subdir/.gitignore"),
                WorkspaceMetadata {
                    is_dir: false,
                    is_file: true,
                    len: 11,
                    modified_at: None,
                },
            )]),
            file_contents: HashMap::from([(
                PathBuf::from("/srv/workspace/src/subdir/.gitignore"),
                b"ignored.rs\n".to_vec(),
            )]),
            directories: HashMap::from([(
                PathBuf::from("/srv/workspace/src/subdir"),
                vec![
                    WorkspaceDirectoryEntry {
                        path: PathBuf::from("/srv/workspace/src/subdir/keep.rs"),
                        name: "keep.rs".to_string(),
                        is_dir: false,
                    },
                    WorkspaceDirectoryEntry {
                        path: PathBuf::from("/srv/workspace/src/subdir/ignored.rs"),
                        name: "ignored.rs".to_string(),
                        is_dir: false,
                    },
                ],
            )]),
        };

        let content = list_directory_entries(
            Path::new("/workspace-link"),
            &access,
            Path::new("/srv/workspace/src/subdir"),
            10,
            &CancellationToken::new(),
        )
        .expect("nested directory listing should honor its own gitignore");

        assert_eq!(content, "keep.rs");
    }
}
