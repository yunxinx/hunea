use std::{
    collections::BTreeMap,
    fmt,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

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

/// `AcpInitializeOutcome` 表示 ACP initialize 握手后的 agent 基本信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpInitializeOutcome {
    pub protocol_version: agent_client_protocol::schema::ProtocolVersion,
    pub agent_name: Option<String>,
    pub agent_title: Option<String>,
    pub agent_version: Option<String>,
    pub auth_method_count: usize,
}

/// `AcpSessionEvent` 表示后台 ACP 会话 worker 产生的运行事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpSessionEvent {
    Started {
        agent_id: String,
        session_id: String,
        outcome: AcpInitializeOutcome,
    },
    StartFailed {
        agent_id: String,
        message: String,
    },
    PromptStarted {
        agent_id: String,
    },
    PromptResponse {
        agent_id: String,
        content: String,
        stop_reason: String,
    },
    PromptFailed {
        agent_id: String,
        message: String,
    },
    PermissionRequestCancelled {
        agent_id: String,
    },
    Stopped {
        agent_id: String,
        message: Option<String>,
    },
}

#[derive(Debug)]
enum AcpWorkerCommand {
    Prompt(String),
    Shutdown,
}

/// `AcpSessionWorker` 在独立线程中持有 ACP agent 进程与会话。
#[derive(Debug)]
pub struct AcpSessionWorker {
    agent_id: String,
    commands: tokio::sync::mpsc::UnboundedSender<AcpWorkerCommand>,
    events: mpsc::Receiver<AcpSessionEvent>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl AcpSessionWorker {
    /// `start` 启动本地 ACP agent 并异步创建 protocol session。
    pub fn start(command: AcpSessionCommand) -> Self {
        let agent_id = command.agent_id.clone();
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel();
        let thread_agent_id = agent_id.clone();
        let thread_handle = thread::spawn(move || {
            run_worker_thread(thread_agent_id, command, command_rx, event_tx);
        });

        Self {
            agent_id,
            commands: command_tx,
            events: event_rx,
            thread_handle: Some(thread_handle),
        }
    }

    /// `agent_id` 返回当前 worker 绑定的 ACP agent。
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// `send_prompt` 向已启动的 ACP session 发送一轮用户 prompt。
    pub fn send_prompt(&self, prompt: String) -> Result<(), AcpWorkerSendError> {
        self.commands
            .send(AcpWorkerCommand::Prompt(prompt))
            .map_err(|_| AcpWorkerSendError::Closed)
    }

    /// `try_recv_event` 非阻塞读取一个 worker 事件。
    pub fn try_recv_event(&self) -> Option<AcpSessionEvent> {
        self.events.try_recv().ok()
    }

    /// `shutdown` 请求 worker 停止当前 ACP session。
    pub fn shutdown(&mut self) {
        let _ = self.commands.send(AcpWorkerCommand::Shutdown);
        let _ = self.thread_handle.take();
    }
}

impl Drop for AcpSessionWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `AcpWorkerSendError` 描述向 ACP worker 投递命令失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpWorkerSendError {
    Closed,
}

impl fmt::Display for AcpWorkerSendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => write!(f, "ACP session worker is closed"),
        }
    }
}

impl std::error::Error for AcpWorkerSendError {}

/// `AcpHandshakeError` 描述 ACP 协议握手失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpHandshakeError {
    Runtime {
        message: String,
    },
    Spawn {
        agent_id: String,
        message: String,
    },
    MissingPipe {
        agent_id: String,
        pipe: &'static str,
    },
    Timeout {
        agent_id: String,
    },
    Protocol {
        message: String,
    },
}

impl fmt::Display for AcpHandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime { message } => write!(f, "ACP runtime failed: {message}"),
            Self::Spawn { agent_id, message } => {
                write!(f, "spawn ACP agent {agent_id}: {message}")
            }
            Self::MissingPipe { agent_id, pipe } => {
                write!(f, "spawn ACP agent {agent_id}: missing {pipe}")
            }
            Self::Timeout { agent_id } => write!(f, "ACP initialize timed out: {agent_id}"),
            Self::Protocol { message } => write!(f, "ACP initialize failed: {message}"),
        }
    }
}

impl std::error::Error for AcpHandshakeError {}

fn run_worker_thread(
    agent_id: String,
    command: AcpSessionCommand,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
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
        event_tx.clone(),
        Arc::clone(&started),
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
    event_tx: mpsc::Sender<AcpSessionEvent>,
    started: Arc<AtomicBool>,
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
        event_tx,
        started,
    )
    .await;

    let _ = child.kill().await;
    result
}

async fn run_agent_transport_worker<T>(
    agent_id: String,
    transport: T,
    command_rx: tokio::sync::mpsc::UnboundedReceiver<AcpWorkerCommand>,
    event_tx: mpsc::Sender<AcpSessionEvent>,
    started: Arc<AtomicBool>,
) -> Result<(), String>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::Client> + 'static,
{
    use acp::schema::{
        Implementation, InitializeRequest, ProtocolVersion, RequestPermissionOutcome,
        RequestPermissionRequest, RequestPermissionResponse,
    };
    use agent_client_protocol as acp;

    let permission_agent_id = agent_id.clone();
    let permission_event_tx = event_tx.clone();

    acp::Client
        .builder()
        .name("lumos")
        .on_receive_request(
            async move |_request: RequestPermissionRequest, responder, _connection| {
                let _ = permission_event_tx.send(AcpSessionEvent::PermissionRequestCancelled {
                    agent_id: permission_agent_id.clone(),
                });
                responder.respond(RequestPermissionResponse::new(
                    RequestPermissionOutcome::Cancelled,
                ))
            },
            acp::on_receive_request!(),
        )
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

            run_agent_prompt_loop(agent_id.clone(), &mut session, command_rx, event_tx.clone())
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
    event_tx: mpsc::Sender<AcpSessionEvent>,
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

                match read_prompt_response(session).await {
                    Ok((content, stop_reason)) => {
                        let _ = event_tx.send(AcpSessionEvent::PromptResponse {
                            agent_id: agent_id.clone(),
                            content,
                            stop_reason,
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
) -> Result<(String, String), String> {
    use acp::schema::{ContentBlock, ContentChunk, SessionNotification, SessionUpdate};
    use agent_client_protocol as acp;
    use agent_client_protocol::{SessionMessage, util::MatchDispatch};

    let mut output = String::new();
    loop {
        let update = session
            .read_update()
            .await
            .map_err(|error| error.to_string())?;
        match update {
            SessionMessage::SessionMessage(dispatch) => MatchDispatch::new(dispatch)
                .if_notification(async |notification: SessionNotification| {
                    if let SessionUpdate::AgentMessageChunk(ContentChunk {
                        content: ContentBlock::Text(text),
                        ..
                    }) = notification.update
                    {
                        output.push_str(&text.text);
                    }
                    Ok(())
                })
                .await
                .otherwise_ignore()
                .map_err(|error| error.to_string())?,
            SessionMessage::StopReason(stop_reason) => {
                return Ok((output, format!("{stop_reason:?}")));
            }
            _ => {}
        }
    }
}

/// `initialize_agent_command` 启动本地 ACP agent，并通过 stdio 执行 initialize 握手。
pub async fn initialize_agent_command(
    command: &AcpSessionCommand,
) -> Result<AcpInitializeOutcome, AcpHandshakeError> {
    use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

    let mut child = tokio::process::Command::new(&command.command)
        .args(&command.args)
        .envs(&command.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| AcpHandshakeError::Spawn {
            agent_id: command.agent_id.clone(),
            message: error.to_string(),
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| AcpHandshakeError::MissingPipe {
            agent_id: command.agent_id.clone(),
            pipe: "stdin",
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AcpHandshakeError::MissingPipe {
            agent_id: command.agent_id.clone(),
            pipe: "stdout",
        })?;

    let transport = agent_client_protocol::ByteStreams::new(stdin.compat_write(), stdout.compat());
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        initialize_agent_transport(transport),
    )
    .await
    .map_err(|_| AcpHandshakeError::Timeout {
        agent_id: command.agent_id.clone(),
    })??;

    let _ = child.kill().await;
    Ok(outcome)
}

/// `initialize_agent_command_blocking` 在同步调用点执行一次 ACP initialize 探测。
pub fn initialize_agent_command_blocking(
    command: &AcpSessionCommand,
) -> Result<AcpInitializeOutcome, AcpHandshakeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|error| AcpHandshakeError::Runtime {
            message: error.to_string(),
        })?;

    runtime.block_on(initialize_agent_command(command))
}

/// `initialize_agent_transport` 通过给定 transport 执行 ACP initialize 握手。
pub async fn initialize_agent_transport<T>(
    transport: T,
) -> Result<AcpInitializeOutcome, AcpHandshakeError>
where
    T: agent_client_protocol::ConnectTo<agent_client_protocol::Client> + 'static,
{
    use acp::schema::{Implementation, InitializeRequest, ProtocolVersion};
    use agent_client_protocol as acp;

    let response = acp::Client
        .builder()
        .name("lumos")
        .connect_with(transport, async |connection| {
            connection
                .send_request(InitializeRequest::new(ProtocolVersion::LATEST).client_info(
                    Implementation::new("lumos", env!("CARGO_PKG_VERSION")).title("Lumos"),
                ))
                .block_task()
                .await
        })
        .await
        .map_err(|error| AcpHandshakeError::Protocol {
            message: error.to_string(),
        })?;

    Ok(initialize_outcome_from_response(response))
}

fn initialize_outcome_from_response(
    response: agent_client_protocol::schema::InitializeResponse,
) -> AcpInitializeOutcome {
    let agent_info = response.agent_info;
    AcpInitializeOutcome {
        protocol_version: response.protocol_version,
        agent_name: agent_info.as_ref().map(|info| info.name.clone()),
        agent_title: agent_info.as_ref().and_then(|info| info.title.clone()),
        agent_version: agent_info.as_ref().map(|info| info.version.clone()),
        auth_method_count: response.auth_methods.len(),
    }
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
