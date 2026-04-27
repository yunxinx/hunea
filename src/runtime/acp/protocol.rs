use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use super::{
    AcpSessionCommand, AcpSessionEvent,
    handshake::initialize_outcome_from_response,
    permission::{AcpPermissionRegistry, acp_permission_request_from_sdk},
    worker::AcpWorkerCommand,
};

enum AcpPromptReadOutcome {
    Completed(String),
    Interrupted,
}

pub(crate) fn run_worker_thread(
    agent_id: String,
    command: AcpSessionCommand,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
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

    let result = runtime.block_on(run_agent_command_worker(
        command,
        command_rx,
        cancel_rx,
        event_tx.clone(),
        Arc::clone(&started),
        permissions,
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
    event_tx: mpsc::Sender<AcpSessionEvent>,
    started: Arc<AtomicBool>,
    permissions: AcpPermissionRegistry,
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
    let result = run_agent_transport_worker(
        command.agent_id.clone(),
        transport,
        command_rx,
        cancel_rx,
        event_tx,
        started,
        permissions,
    )
    .await;

    let _ = child.kill().await;
    result
}

pub(crate) async fn run_agent_transport_worker<T>(
    agent_id: String,
    transport: T,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    started: Arc<AtomicBool>,
    permissions: AcpPermissionRegistry,
) -> Result<(), String>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::Client> + 'static,
{
    use acp::schema::{Implementation, InitializeRequest, ProtocolVersion};
    use agent_client_protocol as acp;

    acp::Client
        .builder()
        .name("lumos")
        .connect_with(transport, async move |connection| {
            let response = connection
                .send_request(InitializeRequest::new(ProtocolVersion::LATEST).client_info(
                    Implementation::new("lumos", env!("CARGO_PKG_VERSION")).title("Lumos"),
                ))
                .block_task()
                .await?;
            let outcome = initialize_outcome_from_response(response);
            let mut session = connection
                .build_session_cwd()?
                .block_task()
                .start_session()
                .await?;
            let session_id = session.session_id().to_string();
            started.store(true, Ordering::SeqCst);
            event_tx
                .send(AcpSessionEvent::Started {
                    agent_id: agent_id.clone(),
                    session_id,
                    outcome,
                })
                .map_err(|_| acp::Error::internal_error())?;

            run_agent_prompt_loop(
                agent_id.clone(),
                &mut session,
                command_rx,
                cancel_rx,
                event_tx.clone(),
                permissions,
            )
            .await;

            let _ = event_tx.send(AcpSessionEvent::Stopped {
                agent_id,
                message: None,
            });
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())
}

async fn run_agent_prompt_loop(
    agent_id: String,
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    mut command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    mut cancel_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    permissions: AcpPermissionRegistry,
) {
    while let Some(command) = command_rx.recv().await {
        match command {
            AcpWorkerCommand::Prompt(prompt) => {
                let _ = event_tx.send(AcpSessionEvent::PromptStarted {
                    agent_id: agent_id.clone(),
                });
                if let Err(error) = session.send_prompt(prompt) {
                    let _ = event_tx.send(AcpSessionEvent::PromptFailed {
                        agent_id: agent_id.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }

                match read_prompt_response(
                    session,
                    &agent_id,
                    &event_tx,
                    &permissions,
                    &mut cancel_rx,
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
            AcpWorkerCommand::Shutdown => break,
        }
    }
}

async fn read_prompt_response(
    session: &mut agent_client_protocol::ActiveSession<'static, agent_client_protocol::Agent>,
    agent_id: &str,
    event_tx: &mpsc::Sender<AcpSessionEvent>,
    permissions: &AcpPermissionRegistry,
    cancel_rx: &mut tokio::sync::mpsc::UnboundedReceiver<()>,
) -> Result<AcpPromptReadOutcome, String> {
    use acp::schema::{
        ContentBlock, ContentChunk, RequestPermissionOutcome, RequestPermissionRequest,
        RequestPermissionResponse, SelectedPermissionOutcome, SessionNotification, SessionUpdate,
    };
    use agent_client_protocol as acp;
    use agent_client_protocol::{SessionMessage, util::MatchDispatch};

    let mut is_interrupted = false;
    let mut is_cancel_rx_closed = false;
    loop {
        let update = tokio::select! {
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
        };
        match update {
            SessionMessage::SessionMessage(dispatch) => {
                let permission_agent_id = agent_id.to_string();
                let permission_event_tx = event_tx.clone();
                let permission_registry = permissions.clone();
                MatchDispatch::new(dispatch)
                    .if_notification(async |notification: SessionNotification| {
                        if let SessionUpdate::AgentMessageChunk(ContentChunk {
                            content: ContentBlock::Text(text),
                            ..
                        }) = notification.update
                        {
                            let _ = event_tx.send(AcpSessionEvent::AgentMessageChunk {
                                agent_id: agent_id.to_string(),
                                content: text.text,
                            });
                        }
                        Ok(())
                    })
                    .await
                    .if_request(async move |request: RequestPermissionRequest, responder| {
                        let (request_id, response_rx) = permission_registry.register();
                        let permission_request =
                            acp_permission_request_from_sdk(request_id, &request);
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
            }
            SessionMessage::StopReason(stop_reason) => {
                if is_interrupted {
                    return Ok(AcpPromptReadOutcome::Interrupted);
                }
                return Ok(AcpPromptReadOutcome::Completed(format!("{stop_reason:?}")));
            }
            _ => {}
        }
    }
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
