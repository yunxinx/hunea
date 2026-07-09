//! Managed search 工具的 workspace tool registry 构建。

use std::path::{Path, PathBuf};

use tool_runtime::{
    ToolExecutorRegistry,
    builtin::{
        ManagedSearchToolConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
    },
};

pub(crate) fn conversation_workspace_tools(
    managed_search_tools: &ManagedSearchToolConfig,
    managed_root: &Path,
) -> ToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_tool_registry_with_options(
        root,
        WorkspaceToolRegistryOptions {
            managed_search_tools: managed_search_tools.clone(),
            managed_root: managed_root.to_path_buf(),
        },
    )
}
