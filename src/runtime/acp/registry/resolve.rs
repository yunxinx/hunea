use std::{collections::BTreeMap, fmt};

use crate::appconfig::{AgentServerConfig, AgentServerType};

use super::model::{RegistryAgent, RegistryBinaryTarget, RegistryDocument};

/// `ResolvedAcpCommand` 是 registry binary target 解析后的启动信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAcpCommand {
    pub agent_id: String,
    pub agent_name: String,
    pub agent_version: String,
    pub archive_url: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// `RegistryResolveError` 描述 registry 解析到可启动命令时的失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryResolveError {
    AgentNotFound { agent_id: String },
    UnsupportedPlatform,
    BinaryDistributionMissing { agent_id: String },
    BinaryTargetMissing { agent_id: String, platform: String },
    NonRegistryServer { server_id: String },
}

impl fmt::Display for RegistryResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AgentNotFound { agent_id } => {
                write!(f, "ACP registry agent not found: {agent_id}")
            }
            Self::UnsupportedPlatform => write!(
                f,
                "current platform is not supported by ACP registry binary targets"
            ),
            Self::BinaryDistributionMissing { agent_id } => {
                write!(
                    f,
                    "ACP registry agent {agent_id} has no binary distribution"
                )
            }
            Self::BinaryTargetMissing { agent_id, platform } => write!(
                f,
                "ACP registry agent {agent_id} has no binary target for {platform}"
            ),
            Self::NonRegistryServer { server_id } => {
                write!(f, "ACP agent server {server_id} is not registry-backed")
            }
        }
    }
}

impl std::error::Error for RegistryResolveError {}

/// `current_platform_target` 返回 ACP registry 使用的平台 target 名称。
pub fn current_platform_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("darwin-aarch64"),
        ("macos", "x86_64") => Some("darwin-x86_64"),
        ("linux", "aarch64") => Some("linux-aarch64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        ("windows", "aarch64") => Some("windows-aarch64"),
        ("windows", "x86_64") => Some("windows-x86_64"),
        _ => None,
    }
}

/// `resolve_binary_command` 按当前平台解析 registry binary 启动命令。
pub fn resolve_binary_command(
    registry: &RegistryDocument,
    agent_id: &str,
) -> Result<ResolvedAcpCommand, RegistryResolveError> {
    let platform = current_platform_target().ok_or(RegistryResolveError::UnsupportedPlatform)?;
    let agent = find_agent(registry, agent_id)?;
    let empty_env = BTreeMap::new();
    resolve_agent_binary_target(agent, platform, "", &[], &empty_env)
}

/// `resolve_binary_command_for_server` 按 agent server 配置解析 registry 启动命令。
pub fn resolve_binary_command_for_server(
    registry: &RegistryDocument,
    server_id: &str,
    server: &AgentServerConfig,
) -> Result<ResolvedAcpCommand, RegistryResolveError> {
    if server.server_type != AgentServerType::Registry {
        return Err(RegistryResolveError::NonRegistryServer {
            server_id: server_id.to_string(),
        });
    }

    let platform = current_platform_target().ok_or(RegistryResolveError::UnsupportedPlatform)?;
    let agent_id = if server.agent.is_empty() {
        server_id
    } else {
        server.agent.as_str()
    };
    let agent = find_agent(registry, agent_id)?;
    resolve_agent_binary_target(agent, platform, &server.command, &server.args, &server.env)
}

fn resolve_agent_binary_target(
    agent: &RegistryAgent,
    platform: &str,
    configured_command: &str,
    args_override: &[String],
    env_override: &BTreeMap<String, String>,
) -> Result<ResolvedAcpCommand, RegistryResolveError> {
    let targets = agent.distribution.binary.as_ref().ok_or_else(|| {
        RegistryResolveError::BinaryDistributionMissing {
            agent_id: agent.id.clone(),
        }
    })?;
    let target =
        targets
            .get(platform)
            .ok_or_else(|| RegistryResolveError::BinaryTargetMissing {
                agent_id: agent.id.clone(),
                platform: platform.to_string(),
            })?;

    Ok(apply_override(
        agent,
        target,
        configured_command,
        args_override,
        env_override,
    ))
}

fn find_agent<'a>(
    registry: &'a RegistryDocument,
    agent_id: &str,
) -> Result<&'a RegistryAgent, RegistryResolveError> {
    registry
        .agents
        .iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| RegistryResolveError::AgentNotFound {
            agent_id: agent_id.to_string(),
        })
}

fn apply_override(
    agent: &RegistryAgent,
    target: &RegistryBinaryTarget,
    configured_command: &str,
    args_override: &[String],
    env_override: &BTreeMap<String, String>,
) -> ResolvedAcpCommand {
    let mut env = target.env.clone();
    env.extend(env_override.clone());

    ResolvedAcpCommand {
        agent_id: agent.id.clone(),
        agent_name: agent.name.clone(),
        agent_version: agent.version.clone(),
        archive_url: target.archive.clone(),
        command: if configured_command.is_empty() {
            target.cmd.clone()
        } else {
            configured_command.to_string()
        },
        args: if args_override.is_empty() {
            target.args.clone()
        } else {
            args_override.to_vec()
        },
        env,
    }
}
