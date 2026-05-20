use std::path::{Path, PathBuf};

use crate::ToolExecutorRegistry;

use super::workspace_access::{WorkspaceAccess, local_workspace_access};

/// `workspace_readonly_tool_registry` 组合只读 workspace 工具注册表。
pub fn workspace_readonly_tool_registry(root: impl AsRef<Path>) -> ToolExecutorRegistry {
    let root = root.as_ref().to_path_buf();
    let access = local_workspace_access();
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(super::read::read_tool_with_access(&root, access.clone()));
    registry.insert(super::list_dir::list_dir_tool_with_access(&root, access));
    registry
}

pub(crate) fn resolve_workspace_path(
    access: &dyn WorkspaceAccess,
    root: &Path,
    requested: &str,
) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("'path' is required".to_string());
    }

    let root = access
        .canonicalize(root)
        .map_err(|error| format!("workspace root is unavailable: {error}"))?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    let candidate = access
        .canonicalize(&candidate)
        .map_err(|error| format!("path not found: {requested}: {error}"))?;
    if !candidate.starts_with(&root) {
        return Err(format!("path is outside workspace: {requested}"));
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{self, Read},
        path::{Path, PathBuf},
    };

    use super::resolve_workspace_path;
    use crate::builtin::workspace_access::{
        WorkspaceAccess, WorkspaceDirectoryEntry, WorkspaceMetadata,
    };

    struct FakeWorkspaceAccess {
        canonical_paths: HashMap<PathBuf, PathBuf>,
    }

    impl FakeWorkspaceAccess {
        fn new(canonical_paths: impl IntoIterator<Item = (PathBuf, PathBuf)>) -> Self {
            Self {
                canonical_paths: canonical_paths.into_iter().collect(),
            }
        }
    }

    impl WorkspaceAccess for FakeWorkspaceAccess {
        fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
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
    fn resolve_workspace_path_uses_workspace_access_canonicalization() {
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
            resolve_workspace_path(&access, Path::new("/workspace-link"), "src/lib.rs").unwrap();

        assert_eq!(resolved, PathBuf::from("/srv/workspace/src/lib.rs"));
    }

    #[test]
    fn resolve_workspace_path_rejects_paths_outside_workspace_after_backend_resolution() {
        let access = FakeWorkspaceAccess::new([
            (
                PathBuf::from("/workspace-link"),
                PathBuf::from("/srv/workspace"),
            ),
            (PathBuf::from("/etc/passwd"), PathBuf::from("/etc/passwd")),
        ]);

        let error = resolve_workspace_path(&access, Path::new("/workspace-link"), "/etc/passwd")
            .expect_err("outside path should be rejected");

        assert_eq!(error, "path is outside workspace: /etc/passwd");
    }
}
