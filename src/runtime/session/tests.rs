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

#[tokio::test(flavor = "current_thread")]
async fn initialize_agent_over_acp_transport_returns_agent_info() {
    use acp::schema::{Implementation, InitializeRequest, InitializeResponse};
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    tokio::task::spawn(async move {
        acp::Agent
            .builder()
            .on_receive_request(
                async |request: InitializeRequest, responder, _connection| {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version).agent_info(
                            Implementation::new("fake-agent", "0.1.0").title("Fake Agent"),
                        ),
                    )
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let outcome = crate::runtime::session::initialize_agent_transport(client_transport)
        .await
        .expect("initialize should succeed");

    assert_eq!(outcome.agent_name.as_deref(), Some("fake-agent"));
    assert_eq!(outcome.agent_title.as_deref(), Some("Fake Agent"));
    assert_eq!(outcome.agent_version.as_deref(), Some("0.1.0"));
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_agent_command_reports_spawn_failure() {
    let command = crate::runtime::session::AcpSessionCommand {
        agent_id: "missing".to_string(),
        command: "lumos-definitely-missing-acp-agent".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        default_model: None,
        default_mode: None,
    };

    let error = crate::runtime::session::initialize_agent_command(&command)
        .await
        .expect_err("missing command should fail before protocol handshake");

    assert!(error.to_string().contains("spawn ACP agent missing"));
}

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_transport_creates_session_and_reads_prompt_response() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        ContentBlock, ContentChunk, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
        SessionUpdate, StopReason, TextContent,
    };
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    tokio::task::spawn(async move {
        acp::Agent
            .builder()
            .on_receive_request(
                async |request: InitializeRequest, responder, _connection| {
                    responder.respond(
                        InitializeResponse::new(request.protocol_version)
                            .agent_info(Implementation::new("fake-agent", "0.1.0")),
                    )
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async |_request: NewSessionRequest, responder, _connection| {
                    responder.respond(NewSessionResponse::new("test-session"))
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async |request: PromptRequest, responder, connection| {
                    connection.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new("pong"),
                        ))),
                    ))?;
                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
    ));

    let started = recv_worker_event(&event_rx, "worker should report session start").await;
    assert!(matches!(
        started,
        super::AcpSessionEvent::Started { ref session_id, .. } if session_id == "test-session"
    ));

    command_tx
        .send(super::AcpWorkerCommand::Prompt("ping".to_string()))
        .expect("prompt command should send");

    let prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;
    assert!(matches!(
        prompt_started,
        super::AcpSessionEvent::PromptStarted { .. }
    ));

    let prompt_response =
        recv_worker_event(&event_rx, "worker should report prompt response").await;
    assert!(matches!(
        prompt_response,
        super::AcpSessionEvent::PromptResponse { ref content, .. } if content == "pong"
    ));

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");

    let stopped = recv_worker_event(&event_rx, "worker should report stop").await;
    assert!(matches!(
        stopped,
        super::AcpSessionEvent::Stopped { message: None, .. }
    ));
}

async fn recv_worker_event(
    event_rx: &std::sync::mpsc::Receiver<super::AcpSessionEvent>,
    context: &str,
) -> super::AcpSessionEvent {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if let Ok(event) = event_rx.try_recv() {
            return event;
        }
        assert!(std::time::Instant::now() < deadline, "{context}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
