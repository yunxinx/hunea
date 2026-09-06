//! Workspace tool registry 构建。

use std::path::{Path, PathBuf};

use tool_runtime::builtin::{
    ManagedRipgrepConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
};

use tool_runtime::{ToolCatalog, ToolCatalogError, ToolRegistration};

use super::agent::SpawnAgentsTool;

pub(crate) fn conversation_workspace_tool_catalog(
    managed_ripgrep: &ManagedRipgrepConfig,
    managed_root: &Path,
    spawn_agents_tool: Option<SpawnAgentsTool>,
) -> Result<(ToolCatalog, ToolRegistration), ToolCatalogError> {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let registry = workspace_tool_registry_with_options(
        root,
        WorkspaceToolRegistryOptions {
            managed_ripgrep: managed_ripgrep.clone(),
            managed_root: managed_root.to_path_buf(),
        },
    );
    let (catalog, registration) = ToolCatalog::adopt_registry("workspace-tools", registry)?;
    let registration = match spawn_agents_tool {
        Some(spawn_agents_tool) => {
            registration.combine(catalog.register("agent-runtime", spawn_agents_tool)?)
        }
        None => registration,
    };
    Ok((catalog, registration))
}
