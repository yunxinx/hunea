use std::collections::BTreeMap;

use crate::{
    appconfig::{AcpConfig, AcpDistribution, AcpInstallRoot, AgentServerConfig, AgentServerType},
    runtime::acp::{AcpSessionResolveError, resolve_session_command},
};

#[test]
fn resolve_custom_agent_session_command() {
    let mut env = BTreeMap::new();
    env.insert("KIMI_AUTH".to_string(), "1".to_string());
    let config = acp_config_with_server(
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
    let config = acp_config_with_server(
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
    let config = acp_config_with_server(
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
    let mut config = acp_config_with_server(
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
        .expect_err("disabled ACP should not resolve commands");

    assert_eq!(error, AcpSessionResolveError::AcpDisabled);
}

fn acp_config_with_server(server_id: &str, server: AgentServerConfig) -> AcpConfig {
    let mut agent_servers = BTreeMap::new();
    agent_servers.insert(server_id.to_string(), server);
    AcpConfig {
        enabled: true,
        registry_url: "https://example.test/registry.json".to_string(),
        install_root: AcpInstallRoot::Config,
        custom_install_dir: std::path::PathBuf::new(),
        distribution_preference: vec![AcpDistribution::Binary],
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
    let config = AcpConfig {
        enabled: true,
        registry_url: "https://example.test/registry.json".to_string(),
        install_root: AcpInstallRoot::Config,
        custom_install_dir: std::path::PathBuf::new(),
        distribution_preference: vec![AcpDistribution::Binary],
        auto_update_check: true,
        agent_servers,
    };

    let catalog = crate::runtime::acp::AcpSessionCatalog::from_acp_config(&config);

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

    let outcome = crate::runtime::acp::initialize_agent_transport(client_transport)
        .await
        .expect("initialize should succeed");

    assert_eq!(outcome.agent_name.as_deref(), Some("fake-agent"));
    assert_eq!(outcome.agent_title.as_deref(), Some("Fake Agent"));
    assert_eq!(outcome.agent_version.as_deref(), Some("0.1.0"));
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_agent_command_reports_spawn_failure() {
    let command = crate::runtime::acp::AcpSessionCommand {
        agent_id: "missing".to_string(),
        command: "lumos-definitely-missing-acp-agent".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
        default_model: None,
        default_mode: None,
    };

    let error = crate::runtime::acp::initialize_agent_command(&command)
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
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
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

    let prompt_chunk = recv_worker_event(&event_rx, "worker should stream prompt chunk").await;
    assert!(matches!(
        prompt_chunk,
        super::AcpSessionEvent::AgentMessageChunk { ref content, .. } if content == "pong"
    ));

    let prompt_response =
        recv_worker_event(&event_rx, "worker should report prompt response").await;
    assert!(matches!(
        prompt_response,
        super::AcpSessionEvent::PromptResponse { ref stop_reason, .. } if stop_reason == "EndTurn"
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

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_transport_forwards_thought_chunks_and_model_config() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        ContentBlock, ContentChunk, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionConfigOption,
        SessionConfigOptionCategory, SessionConfigSelectOption, SessionNotification, SessionUpdate,
        StopReason, TextContent,
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
                        SessionUpdate::ConfigOptionUpdate(acp::schema::ConfigOptionUpdate::new(
                            vec![
                                SessionConfigOption::select(
                                    "model",
                                    "Model",
                                    "kimi-k2",
                                    vec![SessionConfigSelectOption::new("kimi-k2", "Kimi K2")],
                                )
                                .category(SessionConfigOptionCategory::Model),
                            ],
                        )),
                    ))?;
                    connection.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::AgentThoughtChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new("thinking"),
                        ))),
                    ))?;
                    connection.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                            TextContent::new("done"),
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
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
    ));

    let _started = recv_worker_event(&event_rx, "worker should report session start").await;
    command_tx
        .send(super::AcpWorkerCommand::Prompt("ping".to_string()))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let model_event = recv_worker_event(&event_rx, "worker should report current model").await;
    match model_event {
        super::AcpSessionEvent::ModelConfigChanged { config, .. } => {
            assert_eq!(config.config_id, "model");
            assert_eq!(config.current_value, "kimi-k2");
            assert_eq!(config.current_name, "Kimi K2");
            assert_eq!(config.options.len(), 1);
            assert_eq!(config.options[0].value, "kimi-k2");
            assert_eq!(config.options[0].name, "Kimi K2");
        }
        other => panic!("expected model config update, got {other:?}"),
    }

    let thought_event = recv_worker_event(&event_rx, "worker should stream thought chunk").await;
    assert!(matches!(
        thought_event,
        super::AcpSessionEvent::AgentThoughtChunk { ref content, .. } if content == "thinking"
    ));

    let prompt_chunk = recv_worker_event(&event_rx, "worker should stream response chunk").await;
    assert!(matches!(
        prompt_chunk,
        super::AcpSessionEvent::AgentMessageChunk { ref content, .. } if content == "done"
    ));

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_reports_initial_model_config_from_new_session() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        Implementation, InitializeRequest, InitializeResponse, NewSessionRequest,
        NewSessionResponse, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigSelectOption,
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
                    responder.respond(NewSessionResponse::new("test-session").config_options(vec![
                            SessionConfigOption::select(
                                "model",
                                "Model",
                                "kimi-k2",
                                vec![
                                    SessionConfigSelectOption::new("kimi-k2", "Kimi K2"),
                                    SessionConfigSelectOption::new("kimi-k1.5", "Kimi K1.5"),
                                ],
                            )
                            .category(SessionConfigOptionCategory::Model),
                        ]))
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
    ));

    let _started = recv_worker_event(&event_rx, "worker should report session start").await;
    let model_event = recv_worker_event(&event_rx, "worker should report initial model").await;
    match model_event {
        super::AcpSessionEvent::ModelConfigChanged { config, .. } => {
            assert_eq!(config.config_id, "model");
            assert_eq!(config.current_value, "kimi-k2");
            assert_eq!(config.current_name, "Kimi K2");
            assert_eq!(
                config
                    .options
                    .iter()
                    .map(|option| option.value.as_str())
                    .collect::<Vec<_>>(),
                vec!["kimi-k2", "kimi-k1.5"]
            );
        }
        other => panic!("expected initial model config, got {other:?}"),
    }

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_sets_model_config_option_and_reports_updated_options() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        Implementation, InitializeRequest, InitializeResponse, NewSessionRequest,
        NewSessionResponse, SessionConfigOption, SessionConfigOptionCategory,
        SessionConfigSelectOption, SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
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
                async |request: SetSessionConfigOptionRequest, responder, _connection| {
                    assert_eq!(request.config_id.to_string(), "model");
                    assert_eq!(request.value.to_string(), "kimi-k1.5");
                    responder.respond(SetSessionConfigOptionResponse::new(vec![
                        SessionConfigOption::select(
                            "model",
                            "Model",
                            "kimi-k1.5",
                            vec![
                                SessionConfigSelectOption::new("kimi-k2", "Kimi K2"),
                                SessionConfigSelectOption::new("kimi-k1.5", "Kimi K1.5"),
                            ],
                        )
                        .category(SessionConfigOptionCategory::Model),
                    ]))
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
    ));

    let _started = recv_worker_event(&event_rx, "worker should report session start").await;
    command_tx
        .send(super::AcpWorkerCommand::SetConfigOption {
            config_id: "model".to_string(),
            value: "kimi-k1.5".to_string(),
        })
        .expect("config command should send");

    let model_event = recv_worker_event(&event_rx, "worker should report selected model").await;
    match model_event {
        super::AcpSessionEvent::ModelConfigChanged { config, .. } => {
            assert_eq!(config.current_value, "kimi-k1.5");
            assert_eq!(config.current_name, "Kimi K1.5");
        }
        other => panic!("expected selected model config, got {other:?}"),
    }

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");
}

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_cancel_prompt_sends_session_cancel_notification() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use acp::schema::{
        CancelNotification, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, StopReason,
    };
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    let cancel_seen = Arc::new(AtomicBool::new(false));
    let agent_cancel_seen = Arc::clone(&cancel_seen);
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
            .on_receive_notification(
                {
                    let cancel_seen = Arc::clone(&agent_cancel_seen);
                    async move |_notification: CancelNotification, _connection| {
                        cancel_seen.store(true, Ordering::SeqCst);
                        Ok(())
                    }
                },
                acp::on_receive_notification!(),
            )
            .on_receive_request(
                async move |_request: PromptRequest, responder, _connection| {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    responder.respond(PromptResponse::new(StopReason::Cancelled))
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
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

    cancel_tx.send(()).expect("cancel command should send");

    let interrupted = recv_worker_event(&event_rx, "worker should report interruption").await;
    assert!(matches!(
        interrupted,
        super::AcpSessionEvent::PromptInterrupted { .. }
    ));
    wait_until(
        || cancel_seen.load(Ordering::SeqCst),
        "agent should receive session/cancel",
    )
    .await;

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");
}

async fn wait_until(condition: impl Fn() -> bool, context: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if condition() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "{context}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
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

#[tokio::test(flavor = "current_thread")]
async fn acp_worker_round_trips_permission_selection() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        ContentBlock, ContentChunk, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind,
        PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
        SessionNotification, SessionUpdate, StopReason, TextContent, ToolCallUpdate,
        ToolCallUpdateFields,
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
                    let session_id = request.session_id.clone();
                    let response_connection = connection.clone();
                    connection
                        .send_request(RequestPermissionRequest::new(
                            request.session_id.clone(),
                            ToolCallUpdate::new(
                                "tool-1",
                                ToolCallUpdateFields::new().title("Write file"),
                            ),
                            vec![
                                PermissionOption::new(
                                    "allow-once",
                                    "Allow once",
                                    PermissionOptionKind::AllowOnce,
                                ),
                                PermissionOption::new(
                                    "reject-once",
                                    "Reject once",
                                    PermissionOptionKind::RejectOnce,
                                ),
                            ],
                        ))
                        .on_receiving_result(async move |permission| {
                            let permission = permission?;
                            let text = match permission.outcome {
                                RequestPermissionOutcome::Selected(selected)
                                    if selected.option_id.to_string() == "allow-once" =>
                                {
                                    "permission granted"
                                }
                                _ => "permission denied",
                            };
                            response_connection.send_notification(SessionNotification::new(
                                session_id,
                                SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                    ContentBlock::Text(TextContent::new(text)),
                                )),
                            ))?;
                            responder.respond(PromptResponse::new(StopReason::EndTurn))
                        })?;
                    Ok(())
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let permissions = super::AcpPermissionRegistry::default();
    let worker = tokio::task::spawn(super::run_agent_transport_worker(
        "fake".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        permissions.clone(),
    ));

    let _started = recv_worker_event(&event_rx, "worker should report session start").await;
    command_tx
        .send(super::AcpWorkerCommand::Prompt("ping".to_string()))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let permission_event = recv_worker_event(&event_rx, "worker should request permission").await;
    let request_id = match permission_event {
        super::AcpSessionEvent::PermissionRequested { request, .. } => {
            assert_eq!(request.title.as_deref(), Some("Write file"));
            assert_eq!(request.options.len(), 2);
            request.request_id
        }
        other => panic!("expected permission request, got {other:?}"),
    };
    permissions
        .respond(&request_id, Some("allow-once".to_string()))
        .expect("permission response should be accepted");

    let prompt_chunk = recv_worker_event(&event_rx, "worker should stream granted chunk").await;
    assert!(matches!(
        prompt_chunk,
        super::AcpSessionEvent::AgentMessageChunk { ref content, .. } if content == "permission granted"
    ));

    command_tx
        .send(super::AcpWorkerCommand::Shutdown)
        .expect("shutdown command should send");
    worker
        .await
        .expect("worker task should join")
        .expect("worker should stop cleanly");
}
