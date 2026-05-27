//! Managed search 工具授权持久化与 workspace tool registry 刷新。

use std::path::PathBuf;

use app_config::appconfig;
use runtime_domain::session::{ManagedSearchTool, RuntimeEvent, RuntimeTarget};
use tool_runtime::{
    ToolExecutorRegistry,
    builtin::{
        ManagedSearchToolConfig, WorkspaceToolRegistryOptions, workspace_tool_registry_with_options,
    },
};

use crate::runtime::AppRuntimeOptions;

pub(crate) fn conversation_workspace_tools(
    managed_search_tools: &ManagedSearchToolConfig,
) -> ToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_tool_registry_with_options(
        root,
        WorkspaceToolRegistryOptions {
            managed_search_tools: managed_search_tools.clone(),
        },
    )
}

pub(crate) fn persist_managed_search_tool_authorization(
    options: &mut AppRuntimeOptions,
    workspace_tools: &mut ToolExecutorRegistry,
    tool: ManagedSearchTool,
    target: Option<RuntimeTarget>,
) -> Option<RuntimeEvent> {
    let Some(path) = options.managed_search_authorization_config_path.as_deref() else {
        return Some(RuntimeEvent::SystemMessage {
            target,
            message: format!(
                "{} installed, but managed download authorization was not saved: user config path is unavailable",
                tool.binary_name()
            ),
        });
    };

    match appconfig::persist_managed_search_tool_authorization_to_path(path, tool) {
        Ok(()) => {
            mark_managed_search_tool_authorized(&mut options.managed_search_tools, tool);
            *workspace_tools = conversation_workspace_tools(&options.managed_search_tools);
            None
        }
        Err(error) => Some(RuntimeEvent::SystemMessage {
            target,
            message: format!(
                "{} installed, but managed download authorization was not saved: {error}",
                tool.binary_name()
            ),
        }),
    }
}

fn mark_managed_search_tool_authorized(
    config: &mut ManagedSearchToolConfig,
    tool: ManagedSearchTool,
) {
    match tool {
        ManagedSearchTool::Ripgrep => config.allow_managed_rg = Some(true),
        ManagedSearchTool::Fd => config.allow_managed_fd = Some(true),
    }
}
