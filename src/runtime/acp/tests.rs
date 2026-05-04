use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use crate::{
    appconfig::{AcpConfig, AcpDistribution, AcpInstallRoot, AgentServerConfig, AgentServerType},
    runtime::acp::{
        AcpAgentIdentity, AcpPromptBlock, AcpSessionResolveError,
        build_acp_prompt_from_composer_text, resolve_session_command,
    },
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
fn acp_agent_identity_reports_prompt_capabilities() {
    use agent_client_protocol::schema::{AgentCapabilities, PromptCapabilities};

    let identity = AcpAgentIdentity {
        agent_capabilities: AgentCapabilities::new().prompt_capabilities(
            PromptCapabilities::new()
                .image(true)
                .audio(true)
                .embedded_context(true),
        ),
        ..AcpAgentIdentity::default()
    };

    assert!(identity.supports_image());
    assert!(identity.supports_audio());
    assert!(identity.supports_embedded_context());
}

#[test]
fn acp_prompt_builder_embeds_supported_image_audio_and_text_resources() {
    use agent_client_protocol::schema::{
        AgentCapabilities, ContentBlock, EmbeddedResourceResource, PromptCapabilities,
    };

    let root = AcpPromptTempFileTree::new("rich-blocks");
    root.write_file("assets/sample.png", &[0x89, b'P', b'N', b'G']);
    root.write_file("audio/sample.wav", b"RIFF");
    root.write_file("src/code.py", b"print('hi')\n");
    let identity = AcpAgentIdentity {
        agent_capabilities: AgentCapabilities::new().prompt_capabilities(
            PromptCapabilities::new()
                .image(true)
                .audio(true)
                .embedded_context(true),
        ),
        ..AcpAgentIdentity::default()
    };

    let prompt = build_acp_prompt_from_composer_text(
        "review @assets/sample.png @audio/sample.wav @src/code.py",
        root.path(),
        &identity,
    );

    let blocks = prompt.to_content_blocks();
    assert_eq!(blocks.len(), 6);
    assert!(matches!(
        &blocks[0],
        ContentBlock::Text(text) if text.text == "review "
    ));
    assert!(matches!(
        &blocks[1],
        ContentBlock::Image(image)
            if image.mime_type == "image/png"
                && image.data == "iVBORw=="
                && image.uri.as_deref() == Some(root.file_uri("assets/sample.png").as_str())
    ));
    assert!(matches!(
        &blocks[3],
        ContentBlock::Audio(audio)
            if audio.mime_type == "audio/wav" && audio.data == "UklGRg=="
    ));
    match &blocks[5] {
        ContentBlock::Resource(resource) => match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(text) => {
                assert_eq!(text.uri, root.file_uri("src/code.py"));
                assert_eq!(text.mime_type.as_deref(), Some("text/x-python"));
                assert_eq!(text.text, "print('hi')\n");
            }
            other => panic!("expected embedded text resource, got {other:?}"),
        },
        other => panic!("expected resource block, got {other:?}"),
    }
}

#[test]
fn acp_prompt_builder_downgrades_missing_optional_capability_to_resource_link() {
    use agent_client_protocol::schema::ContentBlock;

    let root = AcpPromptTempFileTree::new("baseline-link");
    root.write_file("assets/sample.png", &[0x89, b'P', b'N', b'G']);

    let prompt = build_acp_prompt_from_composer_text(
        "inspect @assets/sample.png",
        root.path(),
        &AcpAgentIdentity::default(),
    );

    let blocks = prompt.to_content_blocks();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        &blocks[1],
        ContentBlock::ResourceLink(link)
            if link.name == "sample.png"
                && link.uri == root.file_uri("assets/sample.png")
                && link.mime_type.as_deref() == Some("image/png")
                && link.size == Some(4)
    ));
}

#[test]
fn acp_prompt_builder_downgrades_unreadable_text_resource_to_resource_link() {
    use agent_client_protocol::schema::{AgentCapabilities, ContentBlock, PromptCapabilities};

    let root = AcpPromptTempFileTree::new("non-utf8-resource");
    root.write_file("src/binary.py", &[0xff, 0xfe, 0xfd]);
    let identity = AcpAgentIdentity {
        agent_capabilities: AgentCapabilities::new()
            .prompt_capabilities(PromptCapabilities::new().embedded_context(true)),
        ..AcpAgentIdentity::default()
    };

    let prompt =
        build_acp_prompt_from_composer_text("inspect @src/binary.py", root.path(), &identity);

    let blocks = prompt.to_content_blocks();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(
        &blocks[1],
        ContentBlock::ResourceLink(link)
            if link.name == "binary.py"
                && link.uri == root.file_uri("src/binary.py")
                && link.mime_type.as_deref() == Some("text/x-python")
                && link.size == Some(3)
    ));
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
    use acp::schema::{
        AgentCapabilities, Implementation, InitializeRequest, InitializeResponse,
        PromptCapabilities,
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
                            .agent_capabilities(
                                AgentCapabilities::new()
                                    .load_session(true)
                                    .prompt_capabilities(
                                        PromptCapabilities::new().image(true).audio(true),
                                    ),
                            )
                            .agent_info(
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
    assert!(outcome.agent_capabilities.load_session);
    assert!(outcome.agent_capabilities.prompt_capabilities.image);
    assert!(outcome.agent_capabilities.prompt_capabilities.audio);
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
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
async fn acp_worker_reports_protocol_warning_even_when_new_session_fails() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{InitializeRequest, InitializeResponse, NewSessionRequest, ProtocolVersion};
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    tokio::task::spawn(async move {
        acp::Agent
            .builder()
            .on_receive_request(
                async |_request: InitializeRequest, responder, _connection| {
                    responder.respond(InitializeResponse::new(ProtocolVersion::V0))
                },
                acp::on_receive_request!(),
            )
            .on_receive_request(
                async |_request: NewSessionRequest, responder, _connection| {
                    responder.respond_with_error(acp::Error::internal_error())
                },
                acp::on_receive_request!(),
            )
            .connect_to(agent_transport)
            .await
    });

    let (_command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::channel();
    let result = super::run_agent_transport_worker(
        "Kimi Code CLI".to_string(),
        client_transport,
        command_rx,
        cancel_rx,
        event_tx,
        Arc::new(AtomicBool::new(false)),
        super::AcpPermissionRegistry::default(),
    )
    .await;

    assert!(result.is_err());
    let warning = recv_worker_event(&event_rx, "worker should report protocol warning").await;
    assert!(matches!(
        warning,
        super::AcpSessionEvent::SystemMessage {
            ref agent_id,
            ref message,
        } if agent_id == "Kimi Code CLI"
            && message.contains("ACP protocol version mismatch")
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let model_event = recv_worker_event(&event_rx, "worker should report current model").await;
    match model_event {
        super::AcpSessionEvent::ModelConfigChanged { config, .. } => {
            assert_eq!(config.config_id.as_deref(), Some("model"));
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
async fn acp_worker_transport_forwards_available_commands_update() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, Implementation,
        InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse,
        PromptRequest, PromptResponse, SessionNotification, SessionUpdate, StopReason,
        UnstructuredCommandInput,
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
                        SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(vec![
                            AvailableCommand::new("web", "Search the web").input(
                                AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                                    "query to search for",
                                )),
                            ),
                            AvailableCommand::new("test", "Run tests"),
                        ])),
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let available_commands_event =
        recv_worker_event(&event_rx, "worker should report available commands").await;
    match available_commands_event {
        super::AcpSessionEvent::AvailableCommandsChanged { commands, .. } => {
            assert_eq!(commands.len(), 2);
            assert_eq!(commands[0].name, "web");
            assert_eq!(commands[0].description, "Search the web");
            assert!(matches!(
                commands[0].input,
                Some(super::AcpAvailableCommandInput::Unstructured { ref hint }) if hint == "query to search for"
            ));
            assert_eq!(commands[1].name, "test");
            assert_eq!(commands[1].description, "Run tests");
            assert!(commands[1].input.is_none());
        }
        other => panic!("expected available commands update, got {other:?}"),
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
async fn acp_worker_transport_forwards_tool_call_lifecycle_updates() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        ContentBlock, ContentChunk, Diff, Implementation, InitializeRequest, InitializeResponse,
        NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, SessionNotification,
        SessionUpdate, StopReason, TextContent, ToolCall, ToolCallContent, ToolCallLocation,
        ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
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
                        SessionUpdate::ToolCall(
                            ToolCall::new("call-1", "Reading configuration")
                                .kind(ToolKind::Read)
                                .status(ToolCallStatus::Pending)
                                .locations(vec![ToolCallLocation::new("src/main.rs").line(12)])
                                .raw_input(serde_json::json!({"path": "src/main.rs"})),
                        ),
                    ))?;
                    connection.send_notification(SessionNotification::new(
                        request.session_id.clone(),
                        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                            "call-1",
                            ToolCallUpdateFields::new()
                                .status(ToolCallStatus::Completed)
                                .content(vec![
                                    ToolCallContent::from(ContentBlock::Text(TextContent::new(
                                        "read complete",
                                    ))),
                                    ToolCallContent::from(
                                        Diff::new("src/main.rs", "fn main() {}\n")
                                            .old_text("fn main(){ }\n"),
                                    ),
                                ])
                                .raw_output(serde_json::json!({"ok": true})),
                        )),
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let created = recv_worker_event(&event_rx, "worker should forward tool call").await;
    match created {
        super::AcpSessionEvent::ToolCall { call, .. } => {
            assert_eq!(call.tool_call_id, "call-1");
            assert_eq!(call.title, "Reading configuration");
            assert_eq!(call.kind, super::AcpToolKind::Read);
            assert_eq!(call.status, super::AcpToolCallStatus::Pending);
            assert_eq!(call.locations[0].path, "src/main.rs");
            assert_eq!(call.locations[0].line, Some(12));
            assert!(
                call.raw_input
                    .as_deref()
                    .is_some_and(|raw| raw.contains("src/main.rs"))
            );
        }
        other => panic!("expected tool call event, got {other:?}"),
    }

    let updated = recv_worker_event(&event_rx, "worker should forward tool call update").await;
    match updated {
        super::AcpSessionEvent::ToolCallUpdate { update, .. } => {
            assert_eq!(update.tool_call_id, "call-1");
            assert_eq!(update.status, Some(super::AcpToolCallStatus::Completed));
            assert!(
                update
                    .raw_output
                    .as_deref()
                    .is_some_and(|raw| raw.contains("true"))
            );
            assert_eq!(
                update
                    .content
                    .as_ref()
                    .expect("content should be present")
                    .len(),
                2
            );
        }
        other => panic!("expected tool call update event, got {other:?}"),
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
async fn acp_worker_transport_forwards_idle_available_commands_update() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        AvailableCommand, AvailableCommandsUpdate, Implementation, InitializeRequest,
        InitializeResponse, NewSessionRequest, NewSessionResponse, SessionNotification,
        SessionUpdate,
    };
    use agent_client_protocol as acp;

    let (notify_tx, notify_rx) = tokio::sync::oneshot::channel();
    let notify_rx = Arc::new(tokio::sync::Mutex::new(Some(notify_rx)));
    let (client_transport, agent_transport) = acp::Channel::duplex();
    tokio::task::spawn({
        let notify_rx = Arc::clone(&notify_rx);
        async move {
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
                    async move |_request: NewSessionRequest, responder, connection| {
                        responder.respond(NewSessionResponse::new("test-session"))?;
                        if let Some(notify_rx) = notify_rx.lock().await.take() {
                            let _ = notify_rx.await;
                            connection.send_notification(SessionNotification::new(
                                "test-session",
                                SessionUpdate::AvailableCommandsUpdate(
                                    AvailableCommandsUpdate::new(vec![AvailableCommand::new(
                                        "web",
                                        "Search the web",
                                    )]),
                                ),
                            ))?;
                        }
                        Ok(())
                    },
                    acp::on_receive_request!(),
                )
                .connect_to(agent_transport)
                .await
        }
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
    notify_tx
        .send(())
        .expect("idle update notification should be released");

    let available_commands_event =
        recv_worker_event(&event_rx, "worker should report idle available commands").await;
    match available_commands_event {
        super::AcpSessionEvent::AvailableCommandsChanged { commands, .. } => {
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].name, "web");
            assert_eq!(commands[0].description, "Search the web");
        }
        other => panic!("expected idle available commands update, got {other:?}"),
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
async fn acp_worker_transport_sends_structured_prompt_blocks() {
    use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

    use acp::schema::{
        ContentBlock, Implementation, InitializeRequest, InitializeResponse, NewSessionRequest,
        NewSessionResponse, PromptRequest, PromptResponse, StopReason,
    };
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    let (prompt_tx, prompt_rx) = tokio::sync::oneshot::channel();
    let prompt_tx = Arc::new(Mutex::new(Some(prompt_tx)));
    tokio::task::spawn({
        let prompt_tx = Arc::clone(&prompt_tx);
        async move {
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
                    async move |request: PromptRequest, responder, _connection| {
                        if let Some(sender) = prompt_tx.lock().expect("prompt sender").take() {
                            let _ = sender.send(request.prompt.clone());
                        }
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    },
                    acp::on_receive_request!(),
                )
                .connect_to(agent_transport)
                .await
        }
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_blocks(vec![
                AcpPromptBlock::Text("look ".to_string()),
                AcpPromptBlock::ResourceLink {
                    name: "lib.rs".to_string(),
                    uri: "file:///tmp/lib.rs".to_string(),
                    mime_type: Some("text/x-rust".to_string()),
                    size: Some(12),
                },
            ]),
        ))
        .expect("prompt command should send");

    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;
    let _prompt_response =
        recv_worker_event(&event_rx, "worker should report prompt response").await;
    let prompt = prompt_rx.await.expect("agent should receive prompt");
    assert_eq!(prompt.len(), 2);
    assert!(matches!(
        &prompt[0],
        ContentBlock::Text(text) if text.text == "look "
    ));
    assert!(matches!(
        &prompt[1],
        ContentBlock::ResourceLink(link)
            if link.name == "lib.rs"
                && link.uri == "file:///tmp/lib.rs"
                && link.mime_type.as_deref() == Some("text/x-rust")
                && link.size == Some(12)
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
                                "models",
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
            assert_eq!(config.config_id.as_deref(), Some("models"));
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
async fn acp_worker_reports_initial_model_config_from_legacy_models_state() {
    use std::sync::{Arc, atomic::AtomicBool, mpsc};

    use acp::schema::{
        Implementation, InitializeRequest, InitializeResponse, ModelInfo, NewSessionRequest,
        NewSessionResponse, SessionModelState,
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
                    responder.respond(NewSessionResponse::new("test-session").models(
                        SessionModelState::new(
                            "kimi-for-coding",
                            vec![
                                ModelInfo::new("kimi-for-coding", "Kimi for Coding"),
                                ModelInfo::new(
                                    "kimi-for-coding(thinking)",
                                    "Kimi for Coding (thinking)",
                                ),
                            ],
                        ),
                    ))
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
    let model_event = recv_worker_event(&event_rx, "worker should report legacy models").await;
    match model_event {
        super::AcpSessionEvent::ModelConfigChanged { config, .. } => {
            assert_eq!(config.config_id, None);
            assert_eq!(config.current_value, "kimi-for-coding");
            assert_eq!(config.current_name, "Kimi for Coding");
            assert_eq!(
                config
                    .options
                    .iter()
                    .map(|option| (option.value.as_str(), option.name.as_str()))
                    .collect::<Vec<_>>(),
                vec![
                    ("kimi-for-coding", "Kimi for Coding"),
                    ("kimi-for-coding(thinking)", "Kimi for Coding (thinking)")
                ]
            );
        }
        other => panic!("expected legacy model config, got {other:?}"),
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
        .send(super::AcpWorkerCommand::SetModel {
            config_id: Some("model".to_string()),
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
async fn acp_worker_sets_legacy_model_with_set_session_model_request() {
    use std::sync::{Arc, Mutex, atomic::AtomicBool, mpsc};

    use acp::schema::{
        Implementation, InitializeRequest, InitializeResponse, NewSessionRequest,
        NewSessionResponse, SetSessionModelRequest, SetSessionModelResponse,
    };
    use agent_client_protocol as acp;

    let (client_transport, agent_transport) = acp::Channel::duplex();
    let (selected_model_tx, selected_model_rx) = tokio::sync::oneshot::channel();
    let selected_model_tx = Arc::new(Mutex::new(Some(selected_model_tx)));
    tokio::task::spawn({
        let selected_model_tx = Arc::clone(&selected_model_tx);
        async move {
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
                    async move |request: SetSessionModelRequest, responder, _connection| {
                        let model_id = request.model_id.to_string();
                        assert_eq!(model_id, "kimi-for-coding(thinking)");
                        if let Some(sender) = selected_model_tx
                            .lock()
                            .expect("selected model sender")
                            .take()
                        {
                            let _ = sender.send(model_id);
                        }
                        responder.respond(SetSessionModelResponse::new())
                    },
                    acp::on_receive_request!(),
                )
                .connect_to(agent_transport)
                .await
        }
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
        .send(super::AcpWorkerCommand::SetModel {
            config_id: None,
            value: "kimi-for-coding(thinking)".to_string(),
        })
        .expect("legacy model command should send");

    let selected_model = selected_model_rx
        .await
        .expect("agent should receive legacy model change");
    assert_eq!(selected_model, "kimi-for-coding(thinking)");
    let success_event =
        recv_worker_event(&event_rx, "worker should report model change success").await;
    match success_event {
        super::AcpSessionEvent::ConfigChangeSucceeded { ref agent_id } if agent_id == "fake" => {}
        other => panic!("expected model change success, got {other:?}"),
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
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

struct AcpPromptTempFileTree {
    path: PathBuf,
}

impl AcpPromptTempFileTree {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("lumos-acp-prompt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp root should be creatable");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write_file(&self, relative: &str, content: &[u8]) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("temp parent should be creatable");
        }
        std::fs::write(path, content).expect("temp file should be writable");
    }

    fn file_uri(&self, relative: &str) -> String {
        url::Url::from_file_path(self.path.join(relative))
            .expect("temp file path should convert to URI")
            .to_string()
    }
}

impl Drop for AcpPromptTempFileTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
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
        .send(super::AcpWorkerCommand::Prompt(
            super::AcpPrompt::from_text("ping"),
        ))
        .expect("prompt command should send");
    let _prompt_started = recv_worker_event(&event_rx, "worker should report prompt start").await;

    let permission_event = recv_worker_event(&event_rx, "worker should request permission").await;
    let request_id = match permission_event {
        super::AcpSessionEvent::PermissionRequested { request, .. } => {
            assert_eq!(request.title.as_deref(), Some("Write file"));
            assert_eq!(request.tool_call.tool_call_id, "tool-1");
            assert_eq!(request.tool_call.title.as_deref(), Some("Write file"));
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
