//! Workspace tool registry 构建。

use std::path::{Path, PathBuf};

use tool_runtime::builtin::{
    ManagedRipgrepConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
};

use tool_runtime::{ToolCatalog, ToolCatalogError, ToolRegistration};

pub(crate) fn conversation_workspace_tool_catalog(
    managed_ripgrep: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> Result<(ToolCatalog, ToolRegistration), ToolCatalogError> {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let registry = workspace_tool_registry_with_options(
        root,
        WorkspaceToolRegistryOptions {
            managed_ripgrep: managed_ripgrep.clone(),
            managed_root: managed_root.to_path_buf(),
        },
    );
    ToolCatalog::adopt_registry("workspace-tools", registry)
}
