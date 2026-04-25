use std::{collections::BTreeMap, fmt};

use crate::appconfig::{AgentServerConfig, AgentServerType, RuntimeConfig};

/// `AcpSessionCommand` 描述启动一个本地 ACP agent 进程所需的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpSessionCommand {
    pub agent_id: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub default_model: Option<String>,
    pub default_mode: Option<String>,
}

/// `AcpSessionCatalog` 保存当前 runner 可直接启动的 ACP agent 命令。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcpSessionCatalog {
    commands: BTreeMap<String, AcpSessionCommand>,
}

impl AcpSessionCatalog {
    /// `from_runtime_config` 从 runtime 配置收集无需安装即可启动的 agent。
    pub fn from_runtime_config(config: &RuntimeConfig) -> Self {
        let mut commands = BTreeMap::new();
        for agent_id in config.agent_servers.keys() {
            if let Ok(command) = resolve_session_command(config, agent_id) {
                commands.insert(agent_id.clone(), command);
            }
        }

        Self { commands }
    }

    /// `command` 返回指定 agent 的本地启动命令。
    pub fn command(&self, agent_id: &str) -> Option<&AcpSessionCommand> {
        self.commands.get(agent_id)
    }
}

/// `AcpSessionResolveError` 描述 ACP 会话启动命令无法解析的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpSessionResolveError {
    RuntimeDisabled,
    AgentServerNotFound { agent_id: String },
    CustomCommandMissing { agent_id: String },
    RegistryInstallRequired { agent_id: String },
}

impl fmt::Display for AcpSessionResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RuntimeDisabled => write!(f, "ACP runtime is disabled"),
            Self::AgentServerNotFound { agent_id } => {
                write!(f, "ACP agent server not found: {agent_id}")
            }
            Self::CustomCommandMissing { agent_id } => {
                write!(f, "ACP custom agent server {agent_id} has no command")
            }
            Self::RegistryInstallRequired { agent_id } => {
                write!(f, "ACP registry agent {agent_id} needs installation")
            }
        }
    }
}

impl std::error::Error for AcpSessionResolveError {}

/// `resolve_session_command` 根据 ACP 配置解析本次会话可直接启动的命令。
pub fn resolve_session_command(
    config: &RuntimeConfig,
    agent_id: &str,
) -> Result<AcpSessionCommand, AcpSessionResolveError> {
    if !config.enabled {
        return Err(AcpSessionResolveError::RuntimeDisabled);
    }

    let server = config.agent_servers.get(agent_id).ok_or_else(|| {
        AcpSessionResolveError::AgentServerNotFound {
            agent_id: agent_id.to_string(),
        }
    })?;

    match server.server_type {
        AgentServerType::Custom => resolve_local_command(agent_id, server),
        AgentServerType::Registry if !server.command.trim().is_empty() => {
            resolve_local_command(agent_id, server)
        }
        AgentServerType::Registry => Err(AcpSessionResolveError::RegistryInstallRequired {
            agent_id: registry_agent_id(agent_id, server),
        }),
    }
}

fn resolve_local_command(
    agent_id: &str,
    server: &AgentServerConfig,
) -> Result<AcpSessionCommand, AcpSessionResolveError> {
    if server.command.trim().is_empty() {
        return Err(AcpSessionResolveError::CustomCommandMissing {
            agent_id: agent_id.to_string(),
        });
    }

    Ok(AcpSessionCommand {
        agent_id: agent_id.to_string(),
        command: server.command.clone(),
        args: server.args.clone(),
        env: server.env.clone(),
        default_model: server.default_model.clone(),
        default_mode: server.default_mode.clone(),
    })
}

fn registry_agent_id(server_id: &str, server: &AgentServerConfig) -> String {
    if server.agent.is_empty() {
        server_id.to_string()
    } else {
        server.agent.clone()
    }
}

#[cfg(test)]
mod tests;
