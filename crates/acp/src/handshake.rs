use std::{fmt, process::Stdio, time::Duration};

use super::{
    AcpInitializeOutcome, AcpSessionCommand,
    initialize::{build_initialize_request, initialize_outcome_from_response},
};

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
            Self::Runtime { message } => write!(f, "ACP failed: {message}"),
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
    use agent_client_protocol as acp;

    let response = acp::Client
        .builder()
        .name("lumos")
        .connect_with(transport, async |connection| {
            connection
                .send_request(build_initialize_request())
                .block_task()
                .await
        })
        .await
        .map_err(|error| AcpHandshakeError::Protocol {
            message: error.to_string(),
        })?;

    Ok(initialize_outcome_from_response(response))
}
