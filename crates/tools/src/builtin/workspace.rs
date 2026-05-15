use std::path::{Path, PathBuf};

use crate::ToolExecutorRegistry;

/// `workspace_readonly_tool_registry` 组合只读 workspace 工具注册表。
pub fn workspace_readonly_tool_registry(root: impl AsRef<Path>) -> ToolExecutorRegistry {
    let root = root.as_ref().to_path_buf();
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(super::file_read::file_read_tool(&root));
    registry.insert(super::list_dir::list_dir_tool(&root));
    registry
}

pub(crate) fn resolve_workspace_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let requested = requested.trim();
    if requested.is_empty() {
        return Err("'path' is required".to_string());
    }

    let root = root
        .canonicalize()
        .map_err(|error| format!("workspace root is unavailable: {error}"))?;
    let requested_path = Path::new(requested);
    let candidate = if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    };
    let candidate = candidate
        .canonicalize()
        .map_err(|error| format!("path not found: {requested}: {error}"))?;
    if !candidate.starts_with(&root) {
        return Err(format!("path is outside workspace: {requested}"));
    }
    Ok(candidate)
}
