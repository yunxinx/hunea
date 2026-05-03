use std::{fmt, sync::mpsc, thread};

use super::{
    AcpPermissionRespondError, AcpPrompt, AcpSessionCommand, AcpSessionEvent,
    permission::AcpPermissionRegistry, protocol::run_worker_thread,
};

#[derive(Debug)]
pub(crate) enum AcpWorkerCommand {
    Prompt(AcpPrompt),
    SetModel {
        config_id: Option<String>,
        value: String,
    },
    Shutdown,
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
    pub fn send_prompt(&self, prompt: AcpPrompt) -> Result<(), AcpWorkerSendError> {
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

    /// `set_model` 请求 ACP agent 更新当前模型选择。
    pub fn set_model(
        &self,
        config_id: Option<String>,
        value: String,
    ) -> Result<(), AcpWorkerSendError> {
        self.commands
            .send(AcpWorkerCommand::SetModel { config_id, value })
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
