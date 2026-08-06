//! Workspace tool registry 构建。

use std::path::{Path, PathBuf};

use tool_runtime::{
    ToolExecutorRegistry,
    builtin::{
        ManagedRipgrepConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
    },
};

pub(crate) fn conversation_workspace_tools(
    managed_ripgrep: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> ToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_tool_registry_with_options(
        root,
        WorkspaceToolRegistryOptions {
            managed_ripgrep: managed_ripgrep.clone(),
            managed_root: managed_root.to_path_buf(),
        },
    )
}
