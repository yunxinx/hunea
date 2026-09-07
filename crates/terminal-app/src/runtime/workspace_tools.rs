//! Workspace tool registry 构建。

use std::path::{Path, PathBuf};

use tool_runtime::builtin::{
    ManagedRipgrepConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
};

use tool_runtime::{ToolCatalog, ToolCatalogError, ToolRegistration};

use super::agent::{SendAgentMessageTool, SpawnAgentsTool};

pub(crate) fn conversation_workspace_tool_catalog(
    managed_ripgrep: &ManagedRipgrepConfig,
    managed_root: &Path,
    spawn_agents_tool: Option<SpawnAgentsTool>,
    send_agent_message_tool: Option<SendAgentMessageTool>,
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
    // host-owned Agent 工具共享同一 owner registration；缺省（如初始 prompt inventory）
    // 时整体缺席，不注册半套。
    let mut registration = registration;
    if let Some(spawn_agents_tool) = spawn_agents_tool {
        registration = registration.combine(catalog.register("agent-runtime", spawn_agents_tool)?);
    }
    if let Some(send_agent_message_tool) = send_agent_message_tool {
        registration =
            registration.combine(catalog.register("agent-runtime", send_agent_message_tool)?);
    }
    Ok((catalog, registration))
}
