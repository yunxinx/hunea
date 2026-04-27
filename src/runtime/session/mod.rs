use std::{
    collections::BTreeMap,
    fmt,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    AgentMessageChunk {
        agent_id: String,
        content: String,
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
    PromptInterrupted {
        agent_id: String,
    },
    PermissionRequested {
        agent_id: String,
        request: AcpPermissionRequest,
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

enum AcpPromptReadOutcome {
    Completed(String),
    Interrupted,
}

/// `AcpSessionWorker` 在独立线程中持有 ACP agent 进程与会话。
#[derive(Debug)]
pub struct AcpSessionWorker {
    agent_id: String,
    commands: tokio::sync::mpsc::UnboundedSender<AcpWorkerCommand>,
    cancels: tokio::sync::mpsc::UnboundedSender<()>,
    events: mpsc::Receiver<AcpSessionEvent>,
    thread_handle: Option<thread::JoinHandle<()>>,
    permissions: AcpPermissionRegistry,
}

impl AcpSessionWorker {
    /// `start` 启动本地 ACP agent 并异步创建 protocol session。
    pub fn start(command: AcpSessionCommand) -> Self {
        let agent_id = command.agent_id.clone();
        let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = tokio::sync::mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::channel();
        let permissions = AcpPermissionRegistry::default();
        let thread_agent_id = agent_id.clone();
        let thread_permissions = permissions.clone();
        let thread_handle = thread::spawn(move || {
            run_worker_thread(
                thread_agent_id,
                command,
                command_rx,
                cancel_rx,
                event_tx,
                thread_permissions,
            );
        });

        Self {
            agent_id,
            commands: command_tx,
            cancels: cancel_tx,
            events: event_rx,
            thread_handle: Some(thread_handle),
            permissions,
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

    /// `cancel_prompt` 请求 ACP agent 取消当前正在运行的一轮 prompt。
    pub fn cancel_prompt(&self) -> Result<(), AcpWorkerSendError> {
        self.cancels
            .send(())
            .map_err(|_| AcpWorkerSendError::Closed)
    }

    /// `try_recv_event` 非阻塞读取一个 worker 事件。
    pub fn try_recv_event(&self) -> Option<AcpSessionEvent> {
        self.events.try_recv().ok()
    }

    /// `respond_permission` 把用户选择回传给等待中的 ACP 权限请求。
    pub fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), AcpPermissionRespondError> {
        self.permissions.respond(request_id, option_id)
    }

    /// `shutdown` 请求 worker 停止当前 ACP session。
    pub fn shutdown(&mut self) {
        let _ = self.cancels.send(());
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

/// `AcpPermissionRequest` 是传给 TUI 的 ACP 权限确认请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionRequest {
    pub request_id: String,
    pub title: Option<String>,
    pub options: Vec<AcpPermissionOption>,
}

/// `AcpPermissionOption` 描述权限确认里用户可选择的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
}

/// `AcpPermissionOptionKind` 用于 TUI 选择默认允许/拒绝选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct AcpPermissionRegistry {
    inner: Arc<AcpPermissionRegistryInner>,
}

#[derive(Debug, Default)]
struct AcpPermissionRegistryInner {
    next_id: AtomicUsize,
    pending: Mutex<BTreeMap<String, tokio::sync::oneshot::Sender<Option<String>>>>,
}

impl AcpPermissionRegistry {
    fn register(&self) -> (String, tokio::sync::oneshot::Receiver<Option<String>>) {
        let id = format!(
            "permission-{}",
            self.inner.next_id.fetch_add(1, Ordering::SeqCst)
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("ACP permission registry lock should not be poisoned")
            .insert(id.clone(), tx);
        (id, rx)
    }

    pub(crate) fn respond(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), AcpPermissionRespondError> {
        let sender = self
            .inner
            .pending
            .lock()
            .expect("ACP permission registry lock should not be poisoned")
            .remove(request_id)
            .ok_or(AcpPermissionRespondError::NotFound)?;
        sender
            .send(option_id)
            .map_err(|_| AcpPermissionRespondError::Closed)
    }
}

/// `AcpPermissionRespondError` 描述权限确认回传失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpPermissionRespondError {
    NotFound,
    Closed,
}

impl fmt::Display for AcpPermissionRespondError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "ACP permission request not found"),
            Self::Closed => write!(f, "ACP permission request is closed"),
        }
    }
}

impl std::error::Error for AcpPermissionRespondError {}

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

async fn run_agent_transport_worker<T>(
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

fn acp_permission_request_from_sdk(
    request_id: String,
    request: &agent_client_protocol::schema::RequestPermissionRequest,
) -> AcpPermissionRequest {
    AcpPermissionRequest {
        request_id,
        title: request.tool_call.fields.title.clone(),
        options: request
            .options
            .iter()
            .map(|option| AcpPermissionOption {
                option_id: option.option_id.to_string(),
                name: option.name.clone(),
                kind: acp_permission_option_kind(option.kind),
            })
            .collect(),
    }
}

fn acp_permission_option_kind(
    kind: agent_client_protocol::schema::PermissionOptionKind,
) -> AcpPermissionOptionKind {
    use agent_client_protocol::schema::PermissionOptionKind;

    match kind {
        PermissionOptionKind::AllowOnce => AcpPermissionOptionKind::AllowOnce,
        PermissionOptionKind::AllowAlways => AcpPermissionOptionKind::AllowAlways,
        PermissionOptionKind::RejectOnce => AcpPermissionOptionKind::RejectOnce,
        PermissionOptionKind::RejectAlways => AcpPermissionOptionKind::RejectAlways,
        _ => AcpPermissionOptionKind::Unknown,
    }
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
