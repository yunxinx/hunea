use std::{
    io,
    path::{Component, Path, PathBuf},
};

use crate::ToolExecutorRegistry;

use super::super::{
    command::bash,
    search::{ManagedRipgrepConfig, find, grep},
};
use super::{
    error::WorkspaceFileError,
    file_state::WorkspaceReadState,
    mutation::WorkspaceMutationQueue,
    workspace_access::{WorkspaceAccess, local_workspace_access},
};

/// `WorkspaceToolRegistryOptions` 保存 workspace builtin 工具注册时的窄配置。
///
/// `managed_root` 是 managed ripgrep 的下载安装根，源自 `DataDirResolution::config_dir()`
/// （全局 `~/.config/hunea/` 或便携 `<working_dir>/.hunea/`），由调用方注入。
/// 故意不兼容旧版 `~/.hunea` 硬编码根：无迁移、无双路径查找。
/// Default 用 `.hunea` 占位，仅测试路径使用；生产路径必显式设置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceToolRegistryOptions {
    pub managed_ripgrep: ManagedRipgrepConfig,
    pub managed_root: PathBuf,
}

impl Default for WorkspaceToolRegistryOptions {
    fn default() -> Self {
        Self {
            managed_ripgrep: ManagedRipgrepConfig::default(),
            managed_root: PathBuf::from(".hunea"),
        }
    }
}

/// `workspace_readonly_tool_registry` 组合只读 workspace 工具注册表。
pub fn workspace_readonly_tool_registry(root: impl AsRef<Path>) -> ToolExecutorRegistry {
    workspace_readonly_tool_registry_with_options(root, WorkspaceToolRegistryOptions::default())
}

/// `workspace_readonly_tool_registry_with_options` 使用配置组合只读 workspace 工具注册表。
pub fn workspace_readonly_tool_registry_with_options(
    root: impl AsRef<Path>,
    options: WorkspaceToolRegistryOptions,
) -> ToolExecutorRegistry {
    let root = root.as_ref().to_path_buf();
    let access = local_workspace_access();
    let read_state = WorkspaceReadState::default();
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(super::read::read_tool_with_access(
        &root,
        access.clone(),
        read_state,
    ));
    registry.insert(super::view_image::view_image_tool_with_access(
        &root,
        access.clone(),
    ));
    registry.insert(super::list_dir::list_dir_tool_with_access(
        &root,
        access.clone(),
    ));
    registry.insert(grep::grep_tool_with_config(
        &root,
        options.managed_ripgrep.clone(),
        options.managed_root.clone(),
    ));
    registry.insert(find::find_tool(&root));
    registry
}

/// `workspace_tool_registry` 组合 workspace 读写工具注册表。
pub fn workspace_tool_registry(root: impl AsRef<Path>) -> ToolExecutorRegistry {
    workspace_tool_registry_with_options(root, WorkspaceToolRegistryOptions::default())
}

/// `workspace_tool_registry_with_options` 使用配置组合 workspace 读写工具注册表。
pub fn workspace_tool_registry_with_options(
    root: impl AsRef<Path>,
    options: WorkspaceToolRegistryOptions,
) -> ToolExecutorRegistry {
    let root = root.as_ref().to_path_buf();
    let access = local_workspace_access();
    let read_state = WorkspaceReadState::default();
    let mutation_queue = WorkspaceMutationQueue::default();
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(super::read::read_tool_with_access(
        &root,
        access.clone(),
        read_state.clone(),
    ));
    registry.insert(super::view_image::view_image_tool_with_access(
        &root,
        access.clone(),
    ));
    registry.insert(super::list_dir::list_dir_tool_with_access(
        &root,
        access.clone(),
    ));
    registry.insert(grep::grep_tool_with_config(
        &root,
        options.managed_ripgrep.clone(),
        options.managed_root.clone(),
    ));
    registry.insert(find::find_tool(&root));
    registry.insert(super::write::write_tool_with_access(
        &root,
        access.clone(),
        read_state.clone(),
        mutation_queue.clone(),
    ));
    registry.insert(super::edit::edit_tool_with_access(
        &root,
        access,
        read_state,
        mutation_queue,
    ));
    registry.insert(bash::bash_tool(&root));
    registry
}

pub(crate) fn resolve_read_path(
    access: &dyn WorkspaceAccess,
    root: &Path,
    requested: &str,
) -> Result<PathBuf, WorkspaceFileError> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(WorkspaceFileError::MissingPath);
    }

    let root = access
        .canonicalize(root)
        .map_err(|source| WorkspaceFileError::WorkspaceRoot {
            path: root.to_path_buf(),
            source,
        })?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    access
        .canonicalize(&candidate)
        .map_err(|source| WorkspaceFileError::PathNotFound {
            requested: requested.to_string(),
            source,
        })
}

pub(crate) fn resolve_workspace_write_path(
    access: &dyn WorkspaceAccess,
    root: &Path,
    requested: &str,
) -> Result<PathBuf, WorkspaceFileError> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err(WorkspaceFileError::MissingPath);
    }

    let root = access
        .canonicalize(root)
        .map_err(|source| WorkspaceFileError::WorkspaceRoot {
            path: root.to_path_buf(),
            source,
        })?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };

    match access.canonicalize(&candidate) {
        Ok(path) => {
            if path.starts_with(&root) {
                Ok(path)
            } else {
                Err(WorkspaceFileError::PathOutsideWorkspace {
                    requested: requested.to_string(),
                })
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            resolve_missing_workspace_path(access, &root, &candidate, requested)
        }
        Err(source) => Err(WorkspaceFileError::PathNotFound {
            requested: requested.to_string(),
            source,
        }),
    }
}

fn resolve_missing_workspace_path(
    access: &dyn WorkspaceAccess,
    root: &Path,
    candidate: &Path,
    requested: &str,
) -> Result<PathBuf, WorkspaceFileError> {
    reject_parent_components(requested)?;

    let parent = candidate
        .parent()
        .ok_or_else(|| WorkspaceFileError::PathOutsideWorkspace {
            requested: requested.to_string(),
        })?;
    let (raw_parent, canonical_parent) = nearest_existing_ancestor(access, parent, requested)?;
    if !canonical_parent.starts_with(root) {
        return Err(WorkspaceFileError::PathOutsideWorkspace {
            requested: requested.to_string(),
        });
    }

    let suffix = candidate.strip_prefix(&raw_parent).map_err(|_| {
        WorkspaceFileError::PathOutsideWorkspace {
            requested: requested.to_string(),
        }
    })?;
    Ok(canonical_parent.join(suffix))
}

fn nearest_existing_ancestor(
    access: &dyn WorkspaceAccess,
    path: &Path,
    requested: &str,
) -> Result<(PathBuf, PathBuf), WorkspaceFileError> {
    let mut current = path;
    loop {
        match access.canonicalize(current) {
            Ok(path) => return Ok((current.to_path_buf(), path)),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                let Some(parent) = current.parent() else {
                    return Err(WorkspaceFileError::PathNotFound {
                        requested: requested.to_string(),
                        source,
                    });
                };
                current = parent;
            }
            Err(source) => {
                return Err(WorkspaceFileError::PathNotFound {
                    requested: requested.to_string(),
                    source,
                });
            }
        }
    }
}

fn reject_parent_components(requested: &str) -> Result<(), WorkspaceFileError> {
    let has_parent_component = Path::new(requested)
        .components()
        .any(|component| matches!(component, Component::ParentDir));
    if has_parent_component {
        return Err(WorkspaceFileError::PathOutsideWorkspace {
            requested: requested.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{self, Read},
        path::{Path, PathBuf},
    };

    use super::super::workspace_access::{
        WorkspaceAccess, WorkspaceDirectoryEntry, WorkspaceMetadata,
    };
    use super::{super::error::WorkspaceFileError, resolve_read_path};

    struct FakeWorkspaceAccess {
        canonical_paths: HashMap<PathBuf, PathBuf>,
        canonical_errors: HashMap<PathBuf, io::ErrorKind>,
    }

    impl FakeWorkspaceAccess {
        fn new(canonical_paths: impl IntoIterator<Item = (PathBuf, PathBuf)>) -> Self {
            Self {
                canonical_paths: canonical_paths.into_iter().collect(),
                canonical_errors: HashMap::new(),
            }
        }

        fn with_canonical_error(mut self, path: PathBuf, kind: io::ErrorKind) -> Self {
            self.canonical_errors.insert(path, kind);
            self
        }
    }

    impl WorkspaceAccess for FakeWorkspaceAccess {
        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
            if let Some(kind) = self.canonical_errors.get(path) {
                return Err(io::Error::new(*kind, "canonical path is inaccessible"));
            }
            self.canonical_paths
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing canonical path"))
        }

        fn metadata(&self, _path: &Path) -> io::Result<WorkspaceMetadata> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "metadata is not used in this test",
            ))
        }

        fn open_reader(&self, _path: &Path) -> io::Result<Box<dyn Read + Send>> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "open_reader is not used in this test",
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
    fn resolve_read_path_uses_workspace_access_canonicalization() {
        let access = FakeWorkspaceAccess::new([
            (
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            ),
            (
                PathBuf::from("/srv/workspace/src/lib.rs"),
                PathBuf::from("/srv/workspace/src/lib.rs"),
            ),
        ]);

        let resolved =
            resolve_read_path(&access, Path::new("/workspace-link"), "src/lib.rs").unwrap();

        assert_eq!(resolved, PathBuf::from("/srv/workspace/src/lib.rs"));
    }

    #[test]
    fn resolve_read_path_accepts_paths_outside_workspace_after_backend_resolution() {
        let access = FakeWorkspaceAccess::new([
            (
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            ),
            (PathBuf::from("/etc/passwd"), PathBuf::from("/etc/passwd")),
        ]);

        let resolved = resolve_read_path(&access, Path::new("/workspace-link"), "/etc/passwd")
            .expect("read paths outside the workspace should be accepted");

        assert_eq!(resolved, PathBuf::from("/etc/passwd"));
    }

    #[test]
    fn resolve_read_path_preserves_not_found_source() {
        let access = FakeWorkspaceAccess::new([(
            PathBuf::from("/workspace-link"),
            PathBuf::from("/srv/workspace"),
        )]);

        let error = resolve_read_path(&access, Path::new("/workspace-link"), "missing.txt")
            .expect_err("missing path should be rejected");

        let WorkspaceFileError::PathNotFound { requested, source } = error else {
            panic!("missing path should retain the filesystem source error");
        };
        assert_eq!(requested, "missing.txt");
        assert_eq!(source.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn resolve_read_path_preserves_inaccessible_source() {
        let access = FakeWorkspaceAccess::new([(
            PathBuf::from("/workspace-link"),
            PathBuf::from("/srv/workspace"),
        )])
        .with_canonical_error(
            PathBuf::from("/srv/workspace/denied.txt"),
            io::ErrorKind::PermissionDenied,
        );

        let error = resolve_read_path(&access, Path::new("/workspace-link"), "denied.txt")
            .expect_err("inaccessible path should be rejected");

        let WorkspaceFileError::PathNotFound { requested, source } = error else {
            panic!("inaccessible path should retain the filesystem source error");
        };
        assert_eq!(requested, "denied.txt");
        assert_eq!(source.kind(), io::ErrorKind::PermissionDenied);
    }
}
