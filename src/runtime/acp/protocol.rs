use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use agent_client_protocol::schema::{
    AvailableCommand, AvailableCommandInput, ModelInfo, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOptions, SessionModelState,
};

use super::{
    AcpAvailableCommand, AcpAvailableCommandInput, AcpModelConfig, AcpModelOption, AcpPrompt,
    AcpSessionCommand, AcpSessionEvent, AcpToolCall, AcpToolCallContent, AcpToolCallLocation,
    AcpToolCallRawValue, AcpToolCallStatus, AcpToolCallUpdate, AcpToolKind,
    initialize::{
        build_initialize_request, initialize_outcome_from_response, protocol_version_warning,
    },
    permission::{AcpPermissionRegistry, acp_permission_request_from_sdk},
    terminal::{AcpTerminalError, AcpTerminalManager},
    worker::{AcpTerminalControlCommand, AcpWorkerCommand},
};

enum AcpPromptReadOutcome {
    Completed(String),
    Interrupted,
}

type AcpPromptResponseReceiver = tokio::sync::oneshot::Receiver<Result<String, String>>;

#[derive(Clone)]
pub(crate) struct AcpTransportState {
    started: Arc<AtomicBool>,
    permissions: AcpPermissionRegistry,
}

impl AcpTransportState {
    pub(crate) fn new(started: Arc<AtomicBool>, permissions: AcpPermissionRegistry) -> Self {
        Self {
            started,
            permissions,
        }
    }
}

pub(crate) fn run_worker_thread(
    agent_id: String,
    command: AcpSessionCommand,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    terminal_control_rx: tokio::sync::mpsc::UnboundedReceiver<AcpTerminalControlCommand>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    permissions: AcpPermissionRegistry,
) {
    let started = Arc::new(AtomicBool::new(false));
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = event_tx.send(AcpSessionEvent::StartFailed {
                agent_id,
                message: error.to_string(),
            });
            return;
        }
    };

    let transport_state = AcpTransportState::new(Arc::clone(&started), permissions);
    let result = runtime.block_on(run_agent_command_worker(
        command,
        command_rx,
        cancel_rx,
        terminal_control_rx,
        event_tx.clone(),
        transport_state,
    ));

    if let Err(message) = result {
        let event = if started.load(Ordering::SeqCst) {
            AcpSessionEvent::Stopped {
                agent_id,
                message: Some(message),
            }
        } else {
            AcpSessionEvent::StartFailed { agent_id, message }
        };
        let _ = event_tx.send(event);
    }
}

async fn run_agent_command_worker(
    command: AcpSessionCommand,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    terminal_control_rx: tokio::sync::mpsc::UnboundedReceiver<AcpTerminalControlCommand>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    transport_state: AcpTransportState,
) -> Result<(), String> {
    use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

    let mut child = tokio::process::Command::new(&command.command)
        .args(&command.args)
        .envs(&command.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("spawn ACP agent {}: {error}", command.agent_id))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("spawn ACP agent {}: missing stdin", command.agent_id))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("spawn ACP agent {}: missing stdout", command.agent_id))?;

    let transport = agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat());
    let result = run_agent_transport_worker_with_terminal_control(
        command.agent_id.clone(),
        transport,
        command_rx,
        cancel_rx,
        terminal_control_rx,
        event_tx,
        transport_state,
    )
    .await;

    let _ = child.kill().await;
    result
}

#[cfg(test)]
pub(crate) async fn run_agent_transport_worker<T>(
    agent_id: String,
    transport: T,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    transport_state: AcpTransportState,
) -> Result<(), String>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::Client> + 'static,
{
    let (_terminal_control_tx, terminal_control_rx) = tokio::sync::mpsc::unbounded_channel();
    run_agent_transport_worker_with_terminal_control(
        agent_id,
        transport,
        command_rx,
        cancel_rx,
        terminal_control_rx,
        event_tx,
        transport_state,
    )
    .await
}

pub(crate) async fn run_agent_transport_worker_with_terminal_control<T>(
    agent_id: String,
    transport: T,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    terminal_control_rx: tokio::sync::mpsc::UnboundedReceiver<AcpTerminalControlCommand>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    transport_state: AcpTransportState,
) -> Result<(), String>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::Client> + 'static,
{
    use acp::schema::{
        CreateTerminalRequest, KillTerminalRequest, NewSessionRequest, ReleaseTerminalRequest,
        TerminalOutputRequest, WaitForTerminalExitRequest,
    };
    use agent_client_protocol as acp;

    let terminal_manager = AcpTerminalManager::new(agent_id.clone(), event_tx.clone());
    let create_terminal_manager = terminal_manager.clone();
    let output_terminal_manager = terminal_manager.clone();
    let wait_terminal_manager = terminal_manager.clone();
    let kill_terminal_manager = terminal_manager.clone();
    let release_terminal_manager = terminal_manager.clone();
    let shutdown_terminal_manager = terminal_manager.clone();

    acp::Client
        .builder()
        .name("lumos")
        .on_receive_request(
            async move |request: CreateTerminalRequest, responder, _connection| {
                let response = create_terminal_manager.create(request).await;
                match response {
                    Ok(response) => responder.respond(response),
                    Err(error) => {
                        responder.respond_with_error(acp_error_from_terminal_error(error))
                    }
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: TerminalOutputRequest, responder, _connection| {
                match output_terminal_manager.output(request) {
                    Ok(response) => responder.respond(response),
                    Err(error) => {
                        responder.respond_with_error(acp_error_from_terminal_error(error))
                    }
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: WaitForTerminalExitRequest, responder, _connection| {
                match wait_terminal_manager.wait_for_exit(request).await {
                    Ok(response) => responder.respond(response),
                    Err(error) => {
                        responder.respond_with_error(acp_error_from_terminal_error(error))
                    }
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: KillTerminalRequest, responder, _connection| {
                match kill_terminal_manager.kill(request) {
                    Ok(response) => responder.respond(response),
                    Err(error) => {
                        responder.respond_with_error(acp_error_from_terminal_error(error))
                    }
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ReleaseTerminalRequest, responder, _connection| {
                match release_terminal_manager.release(request) {
                    Ok(response) => responder.respond(response),
                    Err(error) => {
                        responder.respond_with_error(acp_error_from_terminal_error(error))
                    }
                }
            },
            acp::on_receive_request!(),
        )
        .connect_with(transport, async move |connection| {
            let terminal_control_task =
                spawn_terminal_control_task(terminal_manager.clone(), terminal_control_rx);
            let response = connection
                .send_request(build_initialize_request())
                .block_task()
                .await?;
            let outcome = initialize_outcome_from_response(response);
            if let Some(message) = protocol_version_warning(&outcome) {
                event_tx
                    .send(AcpSessionEvent::SystemMessage {
                        agent_id: agent_id.clone(),
                        message,
                    })
                    .map_err(|_| acp::Error::internal_error())?;
            }
            let cwd = std::env::current_dir().map_err(|error| {
                acp::Error::internal_error().data(format!("cannot get current directory: {error}"))
            })?;
            let session_response = connection
                .send_request_to(acp::Agent, NewSessionRequest::new(cwd))
                .block_task()
                .await?;
            let initial_model_config =
                acp_model_config_from_config_options(session_response.config_options.as_deref())
                    .or_else(|| acp_model_config_from_models(session_response.models.as_ref()));
            let mut session = connection.attach_session(session_response, Vec::new())?;
            let session_id = session.session_id().to_string();
            transport_state.started.store(true, Ordering::SeqCst);
            event_tx
                .send(AcpSessionEvent::Started {
                    agent_id: agent_id.clone(),
                    session_id,
                    outcome,
                })
                .map_err(|_| acp::Error::internal_error())?;
            if let Some(config) = initial_model_config {
                event_tx
                    .send(AcpSessionEvent::ModelConfigChanged {
                        agent_id: agent_id.clone(),
                        config,
                    })
                    .map_err(|_| acp::Error::internal_error())?;
            }

            let prompt_loop_result = run_agent_prompt_loop(
                agent_id.clone(),
                &mut session,
                command_rx,
                cancel_rx,
                event_tx.clone(),
                transport_state.permissions,
            )
            .await;

            terminal_control_task.abort();
            shutdown_terminal_manager.release_all_for_shutdown();
            prompt_loop_result.map_err(|error| acp::Error::internal_error().data(error))?;

            let _ = event_tx.send(AcpSessionEvent::Stopped {
                agent_id,
                message: None,
            });
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())
}

fn spawn_terminal_control_task(
    terminal_manager: AcpTerminalManager,
    mut terminal_control_rx: tokio::sync::mpsc::UnboundedReceiver<AcpTerminalControlCommand>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = terminal_control_rx.recv().await {
            match command {
                AcpTerminalControlCommand::StopAll => {
                    terminal_manager.kill_all_active();
                }
            }
        }
    })
}

async fn run_agent_prompt_loop(
    agent_id: String,
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    mut command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    mut cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    permissions: AcpPermissionRegistry,
) -> Result<(), String> {
    let mut is_cancel_rx_closed = false;
    loop {
        let command = tokio::select! {
            biased;
            command = command_rx.recv() => command,
            cancel = cancel_rx.recv(), if !is_cancel_rx_closed => {
                if cancel.is_none() {
                    is_cancel_rx_closed = true;
                }
                continue;
            }
            update = session.read_update() => {
                let update = update.map_err(|error| error.to_string())?;
                let _ = handle_acp_session_message(
                    update,
                    &agent_id,
                    &event_tx,
                    &permissions,
                )
                .await?;
                continue;
            }
        };
        let Some(command) = command else {
            break;
        };
        match command {
            AcpWorkerCommand::Prompt(prompt) => {
                let _ = event_tx.send(AcpSessionEvent::PromptStarted {
                    agent_id: agent_id.clone(),
                });
                let prompt_response_rx = match send_acp_prompt_request(session, prompt) {
                    Ok(receiver) => receiver,
                    Err(error) => {
                        let _ = event_tx.send(AcpSessionEvent::PromptFailed {
                            agent_id: agent_id.clone(),
                            message: error,
                        });
                        continue;
                    }
                };

                match read_prompt_response(
                    session,
                    &agent_id,
                    &event_tx,
                    &permissions,
                    &mut cancel_rx,
                    prompt_response_rx,
                )
                .await
                {
                    Ok(AcpPromptReadOutcome::Completed(stop_reason)) => {
                        let _ = event_tx.send(AcpSessionEvent::PromptResponse {
                            agent_id: agent_id.clone(),
                            content: String::new(),
                            stop_reason,
                        });
                    }
                    Ok(AcpPromptReadOutcome::Interrupted) => {
                        let _ = event_tx.send(AcpSessionEvent::PromptInterrupted {
                            agent_id: agent_id.clone(),
                        });
                    }
                    Err(error) => {
                        let _ = event_tx.send(AcpSessionEvent::PromptFailed {
                            agent_id: agent_id.clone(),
                            message: error,
                        });
                    }
                }
            }
            AcpWorkerCommand::SetModel { config_id, value } => {
                match set_acp_model(session, config_id.as_deref(), &value).await {
                    Ok(Some(config_options)) => {
                        if let Some(config) =
                            acp_model_config_from_config_options(Some(&config_options))
                        {
                            let _ = event_tx.send(AcpSessionEvent::ModelConfigChanged {
                                agent_id: agent_id.clone(),
                                config,
                            });
                        } else {
                            let _ = event_tx.send(AcpSessionEvent::ConfigChangeSucceeded {
                                agent_id: agent_id.clone(),
                            });
                        }
                    }
                    Ok(None) => {
                        let _ = event_tx.send(AcpSessionEvent::ConfigChangeSucceeded {
                            agent_id: agent_id.clone(),
                        });
                    }
                    Err(message) => {
                        let _ = event_tx.send(AcpSessionEvent::ConfigChangeFailed {
                            agent_id: agent_id.clone(),
                            message,
                        });
                    }
                }
            }
            AcpWorkerCommand::Shutdown => break,
        }
    }
    Ok(())
}

fn acp_error_from_terminal_error(error: AcpTerminalError) -> agent_client_protocol::Error {
    match error {
        AcpTerminalError::InvalidRequest(message) => {
            agent_client_protocol::Error::invalid_params().data(message)
        }
        AcpTerminalError::NotFound(message) => agent_client_protocol::Error::invalid_params()
            .data(format!("unknown terminalId: {message}")),
        AcpTerminalError::Spawn(message) => {
            agent_client_protocol::Error::internal_error().data(message)
        }
    }
}

fn send_acp_prompt_request(
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    prompt: AcpPrompt,
) -> Result<AcpPromptResponseReceiver, String> {
    use acp::schema::{PromptRequest, PromptResponse};
    use agent_client_protocol as acp;

    let (response_tx, response_rx) = tokio::sync::oneshot::channel();
    session
        .connection()
        .send_request_to(
            acp::Agent,
            PromptRequest::new(session.session_id().clone(), prompt.into_content_blocks()),
        )
        .on_receiving_result(async move |result: Result<PromptResponse, acp::Error>| {
            let result = result
                .map(|response| format!("{:?}", response.stop_reason))
                .map_err(|error| error.to_string());
            let _ = response_tx.send(result);
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    Ok(response_rx)
}

async fn read_prompt_response(
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    agent_id: &str,
    event_tx: &mpsc::Sender<AcpSessionEvent>,
    permissions: &AcpPermissionRegistry,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
    mut prompt_response_rx: AcpPromptResponseReceiver,
) -> Result<AcpPromptReadOutcome, String> {
    let mut is_interrupted = false;
    let mut is_cancel_rx_closed = false;
    loop {
        let update = tokio::select! {
            biased;
            cancel = cancel_rx.recv(), if !is_cancel_rx_closed => {
                if cancel.is_some() {
                    send_acp_cancel_notification(session)?;
                    is_interrupted = true;
                } else {
                    is_cancel_rx_closed = true;
                }
                continue;
            }
            update = session.read_update() => update.map_err(|error| error.to_string())?,
            response = &mut prompt_response_rx => {
                let stop_reason = match response {
                    Ok(Ok(stop_reason)) => stop_reason,
                    Ok(Err(error)) => return Err(error),
                    Err(_) => return Err("ACP prompt response channel closed".to_string()),
                };
                if is_interrupted {
                    return Ok(AcpPromptReadOutcome::Interrupted);
                }
                return Ok(AcpPromptReadOutcome::Completed(stop_reason));
            }
        };
        if let Some(stop_reason) =
            handle_acp_session_message(update, agent_id, event_tx, permissions).await?
        {
            if is_interrupted {
                return Ok(AcpPromptReadOutcome::Interrupted);
            }
            return Ok(AcpPromptReadOutcome::Completed(stop_reason));
        }
    }
}

async fn handle_acp_session_message(
    update: agent_client_protocol::SessionMessage,
    agent_id: &str,
    event_tx: &mpsc::Sender<AcpSessionEvent>,
    permissions: &AcpPermissionRegistry,
) -> Result<Option<String>, String> {
    use acp::schema::{
        ConfigOptionUpdate, ContentBlock, ContentChunk, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
        SessionNotification, SessionUpdate,
    };
    use agent_client_protocol as acp;
    use agent_client_protocol::{SessionMessage, util::MatchDispatch};

    match update {
        SessionMessage::SessionMessage(dispatch) => {
            let permission_agent_id = agent_id.to_string();
            let permission_event_tx = event_tx.clone();
            let permission_registry = permissions.clone();
            MatchDispatch::new(dispatch)
                .if_notification(async |notification: SessionNotification| {
                    match notification.update {
                        SessionUpdate::AgentMessageChunk(ContentChunk {
                            content: ContentBlock::Text(text),
                            ..
                        }) => {
                            let _ = event_tx.send(AcpSessionEvent::AgentMessageChunk {
                                agent_id: agent_id.to_string(),
                                content: text.text,
                            });
                        }
                        SessionUpdate::AgentMessageChunk(_) => {
                            // 当前 TUI transcript 只渲染 agent 文本输出；非文本 block 暂不展示。
                        }
                        SessionUpdate::AgentThoughtChunk(ContentChunk {
                            content: ContentBlock::Text(text),
                            ..
                        }) => {
                            let _ = event_tx.send(AcpSessionEvent::AgentThoughtChunk {
                                agent_id: agent_id.to_string(),
                                content: text.text,
                            });
                        }
                        SessionUpdate::AgentThoughtChunk(_) => {
                            // 当前 reasoning transcript 只接收文本；非文本 thought block 暂不展示。
                        }
                        SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate {
                            config_options,
                            ..
                        }) => {
                            if let Some(config) =
                                acp_model_config_from_config_options(Some(&config_options))
                            {
                                let _ = event_tx.send(AcpSessionEvent::ModelConfigChanged {
                                    agent_id: agent_id.to_string(),
                                    config,
                                });
                            }
                        }
                        SessionUpdate::AvailableCommandsUpdate(update) => {
                            let _ = event_tx.send(AcpSessionEvent::AvailableCommandsChanged {
                                agent_id: agent_id.to_string(),
                                commands: acp_available_commands_from_sdk(
                                    &update.available_commands,
                                ),
                            });
                        }
                        SessionUpdate::ToolCall(call) => {
                            let _ = event_tx.send(AcpSessionEvent::ToolCall {
                                agent_id: agent_id.to_string(),
                                call: acp_tool_call_from_sdk(call),
                            });
                        }
                        SessionUpdate::ToolCallUpdate(update) => {
                            let _ = event_tx.send(AcpSessionEvent::ToolCallUpdate {
                                agent_id: agent_id.to_string(),
                                update: acp_tool_call_update_from_sdk(update),
                            });
                        }
                        _ => {}
                    }
                    Ok(())
                })
                .await
                .if_request(async move |request: RequestPermissionRequest, responder| {
                    let (request_id, response_rx) = permission_registry.register();
                    let tool_call = acp_tool_call_update_from_sdk(request.tool_call.clone());
                    let permission_request =
                        acp_permission_request_from_sdk(request_id, &request, tool_call);
                    let _ = permission_event_tx.send(AcpSessionEvent::PermissionRequested {
                        agent_id: permission_agent_id.clone(),
                        request: permission_request,
                    });

                    let outcome = match response_rx.await {
                        Ok(Some(option_id)) => RequestPermissionOutcome::Selected(
                            SelectedPermissionOutcome::new(option_id),
                        ),
                        Ok(None) | Err(_) => RequestPermissionOutcome::Cancelled,
                    };
                    responder.respond(RequestPermissionResponse::new(outcome))
                })
                .await
                .otherwise_ignore()
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
        SessionMessage::StopReason(stop_reason) => Ok(Some(format!("{stop_reason:?}"))),
        _ => Ok(None),
    }
}

async fn set_acp_model(
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    config_id: Option<&str>,
    value: &str,
) -> Result<Option<Vec<SessionConfigOption>>, String> {
    use acp::schema::{SetSessionConfigOptionRequest, SetSessionModelRequest};
    use agent_client_protocol as acp;

    match config_id {
        Some(config_id) => {
            let response = session
                .connection()
                .send_request_to(
                    acp::Agent,
                    SetSessionConfigOptionRequest::new(
                        session.session_id().clone(),
                        config_id.to_string(),
                        value.to_string(),
                    ),
                )
                .block_task()
                .await
                .map_err(|error| error.to_string())?;
            Ok(Some(response.config_options))
        }
        None => {
            session
                .connection()
                .send_request_to(
                    acp::Agent,
                    SetSessionModelRequest::new(session.session_id().clone(), value.to_string()),
                )
                .block_task()
                .await
                .map_err(|error| error.to_string())?;
            Ok(None)
        }
    }
}

fn acp_available_commands_from_sdk(commands: &[AvailableCommand]) -> Vec<AcpAvailableCommand> {
    commands
        .iter()
        .filter_map(acp_available_command_from_sdk)
        .collect()
}

fn acp_available_command_from_sdk(command: &AvailableCommand) -> Option<AcpAvailableCommand> {
    let name = command.name.trim().trim_start_matches('/').to_string();
    if name.is_empty() {
        return None;
    }

    Some(AcpAvailableCommand {
        name,
        description: command.description.trim().to_string(),
        input: command
            .input
            .as_ref()
            .map(acp_available_command_input_from_sdk),
    })
}

fn acp_available_command_input_from_sdk(input: &AvailableCommandInput) -> AcpAvailableCommandInput {
    match input {
        AvailableCommandInput::Unstructured(input) => AcpAvailableCommandInput::Unstructured {
            hint: input.hint.trim().to_string(),
        },
        _ => AcpAvailableCommandInput::Unknown,
    }
}

fn acp_tool_call_from_sdk(call: agent_client_protocol::schema::ToolCall) -> AcpToolCall {
    AcpToolCall {
        tool_call_id: call.tool_call_id.to_string(),
        title: call.title,
        kind: acp_tool_kind_from_sdk(call.kind),
        status: acp_tool_call_status_from_sdk(call.status),
        content: call
            .content
            .into_iter()
            .map(acp_tool_call_content_from_sdk)
            .collect(),
        locations: call
            .locations
            .into_iter()
            .map(acp_tool_call_location_from_sdk)
            .collect(),
        raw_input: call.raw_input.map(AcpToolCallRawValue::new),
        raw_output: call.raw_output.map(AcpToolCallRawValue::new),
    }
}

fn acp_tool_call_update_from_sdk(
    update: agent_client_protocol::schema::ToolCallUpdate,
) -> AcpToolCallUpdate {
    let fields = update.fields;
    AcpToolCallUpdate {
        tool_call_id: update.tool_call_id.to_string(),
        title: fields.title,
        kind: fields.kind.map(acp_tool_kind_from_sdk),
        status: fields.status.map(acp_tool_call_status_from_sdk),
        content: fields.content.map(|content| {
            content
                .into_iter()
                .map(acp_tool_call_content_from_sdk)
                .collect()
        }),
        locations: fields.locations.map(|locations| {
            locations
                .into_iter()
                .map(acp_tool_call_location_from_sdk)
                .collect()
        }),
        raw_input: fields.raw_input.map(AcpToolCallRawValue::new),
        raw_output: fields.raw_output.map(AcpToolCallRawValue::new),
    }
}

fn acp_tool_kind_from_sdk(kind: agent_client_protocol::schema::ToolKind) -> AcpToolKind {
    use agent_client_protocol::schema::ToolKind;

    match kind {
        ToolKind::Read => AcpToolKind::Read,
        ToolKind::Edit => AcpToolKind::Edit,
        ToolKind::Delete => AcpToolKind::Delete,
        ToolKind::Move => AcpToolKind::Move,
        ToolKind::Search => AcpToolKind::Search,
        ToolKind::Execute => AcpToolKind::Execute,
        ToolKind::Think => AcpToolKind::Think,
        ToolKind::Fetch => AcpToolKind::Fetch,
        ToolKind::SwitchMode => AcpToolKind::SwitchMode,
        ToolKind::Other => AcpToolKind::Other,
        _ => AcpToolKind::Other,
    }
}

fn acp_tool_call_status_from_sdk(
    status: agent_client_protocol::schema::ToolCallStatus,
) -> AcpToolCallStatus {
    use agent_client_protocol::schema::ToolCallStatus;

    match status {
        ToolCallStatus::Pending => AcpToolCallStatus::Pending,
        ToolCallStatus::InProgress => AcpToolCallStatus::InProgress,
        ToolCallStatus::Completed => AcpToolCallStatus::Completed,
        ToolCallStatus::Failed => AcpToolCallStatus::Failed,
        _ => AcpToolCallStatus::Pending,
    }
}

fn acp_tool_call_location_from_sdk(
    location: agent_client_protocol::schema::ToolCallLocation,
) -> AcpToolCallLocation {
    AcpToolCallLocation {
        path: location.path.display().to_string(),
        line: location.line,
    }
}

fn acp_tool_call_content_from_sdk(
    content: agent_client_protocol::schema::ToolCallContent,
) -> AcpToolCallContent {
    use agent_client_protocol::schema::{ContentBlock, EmbeddedResourceResource, ToolCallContent};

    match content {
        ToolCallContent::Content(content) => match content.content {
            ContentBlock::Text(text) => AcpToolCallContent::Text(text.text),
            ContentBlock::Image(image) => AcpToolCallContent::Image {
                mime_type: image.mime_type,
                uri: image.uri,
            },
            ContentBlock::Audio(audio) => AcpToolCallContent::Audio {
                mime_type: audio.mime_type,
            },
            ContentBlock::ResourceLink(resource) => AcpToolCallContent::ResourceLink {
                uri: resource.uri,
                name: resource.name,
                title: resource.title,
            },
            ContentBlock::Resource(resource) => match resource.resource {
                EmbeddedResourceResource::TextResourceContents(resource) => {
                    AcpToolCallContent::Resource {
                        uri: resource.uri,
                        mime_type: resource.mime_type,
                        text: Some(resource.text),
                    }
                }
                EmbeddedResourceResource::BlobResourceContents(resource) => {
                    AcpToolCallContent::Resource {
                        uri: resource.uri,
                        mime_type: resource.mime_type,
                        text: None,
                    }
                }
                _ => AcpToolCallContent::Unknown("resource".to_string()),
            },
            _ => AcpToolCallContent::Unknown("content".to_string()),
        },
        ToolCallContent::Diff(diff) => AcpToolCallContent::Diff {
            path: diff.path.display().to_string(),
            old_text: diff.old_text,
            new_text: diff.new_text,
        },
        ToolCallContent::Terminal(terminal) => AcpToolCallContent::Terminal {
            terminal_id: terminal.terminal_id.to_string(),
        },
        _ => AcpToolCallContent::Unknown("tool_call_content".to_string()),
    }
}

fn acp_model_config_from_config_options(
    options: Option<&[SessionConfigOption]>,
) -> Option<AcpModelConfig> {
    let option = options?
        .iter()
        .filter(|option| {
            matches!(option.category, Some(SessionConfigOptionCategory::Model))
                || matches!(option.id.to_string().as_str(), "model" | "models")
        })
        .find(|option| matches!(option.kind, SessionConfigKind::Select(_)))?;
    let SessionConfigKind::Select(select) = &option.kind else {
        return None;
    };
    let current_value = select.current_value.to_string();
    let current_value = current_value.trim().to_string();
    if current_value.is_empty() {
        return None;
    }
    let options = model_options_from_select_options(&select.options);
    let current_name = options
        .iter()
        .find(|option| option.value == current_value)
        .map(|option| option.name.clone())
        .unwrap_or_else(|| current_value.clone());
    Some(AcpModelConfig {
        config_id: Some(option.id.to_string()),
        current_value,
        current_name,
        options,
    })
}

fn acp_model_config_from_models(models: Option<&SessionModelState>) -> Option<AcpModelConfig> {
    let models = models?;
    let current_value = models.current_model_id.to_string();
    let current_value = current_value.trim().to_string();
    if current_value.is_empty() {
        return None;
    }
    let options = model_options_from_models(&models.available_models);
    let current_name = options
        .iter()
        .find(|option| option.value == current_value)
        .map(|option| option.name.clone())
        .unwrap_or_else(|| current_value.clone());
    Some(AcpModelConfig {
        config_id: None,
        current_value,
        current_name,
        options,
    })
}

fn model_options_from_select_options(options: &SessionConfigSelectOptions) -> Vec<AcpModelOption> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => options
            .iter()
            .map(|option| AcpModelOption {
                value: option.value.to_string(),
                name: option.name.clone(),
            })
            .collect(),
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .map(|option| AcpModelOption {
                value: option.value.to_string(),
                name: option.name.clone(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn model_options_from_models(models: &[ModelInfo]) -> Vec<AcpModelOption> {
    models
        .iter()
        .map(|model| AcpModelOption {
            value: model.model_id.to_string(),
            name: model.name.clone(),
        })
        .collect()
}

fn send_acp_cancel_notification(
    session: &agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
) -> Result<(), String> {
    use agent_client_protocol::{Agent, schema::CancelNotification};

    session
        .connection()
        .send_notification_to(Agent, CancelNotification::new(session.session_id().clone()))
        .map_err(|error| error.to_string())
}
