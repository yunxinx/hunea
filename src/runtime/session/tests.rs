use std::collections::BTreeMap;

use crate::{
    appconfig::{
        AgentServerConfig, AgentServerType, RuntimeConfig, RuntimeDistribution, RuntimeInstallRoot,
    },
    runtime::session::{AcpSessionResolveError, resolve_session_command},
};

#[test]
fn resolve_custom_agent_session_command() {
    let mut env = BTreeMap::new();
    env.insert("KIMI_AUTH".to_string(), "1".to_string());
    let config = runtime_config_with_server(
        "local-kimi",
        AgentServerConfig {
            server_type: AgentServerType::Custom,
            agent: String::new(),
            command: "kimi".to_string(),
            args: vec!["acp".to_string()],
            env,
            default_model: Some("moonshot-v1".to_string()),
            default_mode: Some("plan".to_string()),
        },
    );

    let command =
        resolve_session_command(&config, "local-kimi").expect("custom ACP agent should resolve");

    assert_eq!(command.agent_id, "local-kimi");
    assert_eq!(command.command, "kimi");
    assert_eq!(command.args, vec!["acp"]);
    assert_eq!(command.env.get("KIMI_AUTH"), Some(&"1".to_string()));
    assert_eq!(command.default_model.as_deref(), Some("moonshot-v1"));
    assert_eq!(command.default_mode.as_deref(), Some("plan"));
}

#[test]
fn resolve_registry_agent_with_command_override_as_local_launch() {
    let config = runtime_config_with_server(
        "kimi",
        AgentServerConfig {
            server_type: AgentServerType::Registry,
            agent: "kimi".to_string(),
            command: "kimi".to_string(),
            args: vec!["acp".to_string()],
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        },
    );

    let command = resolve_session_command(&config, "kimi")
        .expect("registry command override should be directly launchable");

    assert_eq!(command.command, "kimi");
    assert_eq!(command.args, vec!["acp"]);
}

#[test]
fn resolve_registry_agent_without_installed_binary_requires_install() {
    let config = runtime_config_with_server(
        "kimi",
        AgentServerConfig {
            server_type: AgentServerType::Registry,
            agent: "kimi".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        },
    );

    let error = resolve_session_command(&config, "kimi")
        .expect_err("registry agent without local command should require install");

    assert_eq!(
        error,
        AcpSessionResolveError::RegistryInstallRequired {
            agent_id: "kimi".to_string(),
        }
    );
}

#[test]
fn resolve_rejects_disabled_runtime() {
    let mut config = runtime_config_with_server(
        "local-kimi",
        AgentServerConfig {
            server_type: AgentServerType::Custom,
            agent: String::new(),
            command: "kimi".to_string(),
            args: vec!["acp".to_string()],
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        },
    );
    config.enabled = false;

    let error = resolve_session_command(&config, "local-kimi")
        .expect_err("disabled ACP runtime should not resolve commands");

    assert_eq!(error, AcpSessionResolveError::RuntimeDisabled);
}

fn runtime_config_with_server(server_id: &str, server: AgentServerConfig) -> RuntimeConfig {
    let mut agent_servers = BTreeMap::new();
    agent_servers.insert(server_id.to_string(), server);
    RuntimeConfig {
        enabled: true,
        registry_url: "https://example.test/registry.json".to_string(),
        install_root: RuntimeInstallRoot::Config,
        custom_install_dir: std::path::PathBuf::new(),
        distribution_preference: vec![RuntimeDistribution::Binary],
        auto_update_check: true,
        agent_servers,
    }
}

#[test]
fn catalog_keeps_directly_launchable_agents_only() {
    let mut agent_servers = BTreeMap::new();
    agent_servers.insert(
        "local-kimi".to_string(),
        AgentServerConfig {
            server_type: AgentServerType::Custom,
            agent: String::new(),
            command: "kimi".to_string(),
            args: vec!["acp".to_string()],
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        },
    );
    agent_servers.insert(
        "registry-kimi".to_string(),
        AgentServerConfig {
            server_type: AgentServerType::Registry,
            agent: "kimi".to_string(),
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            default_model: None,
            default_mode: None,
        },
    );
    let config = RuntimeConfig {
        enabled: true,
        registry_url: "https://example.test/registry.json".to_string(),
        install_root: RuntimeInstallRoot::Config,
        custom_install_dir: std::path::PathBuf::new(),
        distribution_preference: vec![RuntimeDistribution::Binary],
        auto_update_check: true,
        agent_servers,
    };

    let catalog = crate::runtime::session::AcpSessionCatalog::from_runtime_config(&config);

    assert!(catalog.command("local-kimi").is_some());
    assert!(catalog.command("registry-kimi").is_none());
}
