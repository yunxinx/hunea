//! Dedicated blocking workers 管理的 Agent kernel stdio transport。

use std::{
    collections::{BTreeMap, VecDeque},
    ffi::OsString,
    fmt,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use agent_kernel_protocol::{AgentKernelMessage, AgentKernelRequest, AgentKernelResponse};
use stdio_framing::{FrameCodec, FrameError};

use super::{
    AgentKernelConnectError, AgentKernelConnection, AgentKernelEventSink, AgentKernelEventStream,
    AgentKernelRequestTransport, AgentKernelSource, AgentKernelTransportError,
};

const DEFAULT_QUEUE_CAPACITY: usize = 64;
const LATE_REQUEST_TOMBSTONE_CAPACITY: usize = 64;

/// Child launch 只接受 host 明确准备的值。
#[derive(Clone)]
pub struct StdioAgentKernelTransportOptions {
    executable: PathBuf,
    arguments: Vec<OsString>,
    working_directory: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    frame_codec: FrameCodec,
    queue_capacity: usize,
}

impl fmt::Debug for StdioAgentKernelTransportOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioAgentKernelTransportOptions")
            .field("has_executable", &!self.executable.as_os_str().is_empty())
            .field("argument_count", &self.arguments.len())
            .field("has_working_directory", &self.working_directory.is_some())
            .field("environment_count", &self.environment.len())
            .field("max_frame_bytes", &self.frame_codec.max_frame_bytes())
            .field("queue_capacity", &self.queue_capacity)
            .finish()
    }
}

impl StdioAgentKernelTransportOptions {
    /// 创建只包含显式 executable 的 launch options。
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            arguments: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
            frame_codec: FrameCodec::default(),
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
        }
    }

    /// 追加一个显式 argv value。
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// 设置 child working directory。
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// 添加一个显式允许传给 child 的 environment entry。
    pub fn environment(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// 设置单个 Content-Length frame 的最大字节数。
    pub fn max_frame_bytes(
        mut self,
        max_frame_bytes: usize,
    ) -> Result<Self, StdioAgentKernelTransportError> {
        self.frame_codec = FrameCodec::new(max_frame_bytes)
            .map_err(|_| StdioAgentKernelTransportError::InvalidOptions)?;
        Ok(self)
    }

    /// 设置 request 与 unsolicited-event bounded queue capacity。
    pub fn queue_capacity(
        mut self,
        queue_capacity: usize,
    ) -> Result<Self, StdioAgentKernelTransportError> {
        if queue_capacity == 0 {
            return Err(StdioAgentKernelTransportError::InvalidOptions);
        }
        self.queue_capacity = queue_capacity;
        Ok(self)
    }

    fn validate(&self) -> Result<(), StdioAgentKernelTransportError> {
        if self.executable.as_os_str().is_empty() || self.queue_capacity == 0 {
            return Err(StdioAgentKernelTransportError::InvalidOptions);
        }
        Ok(())
    }
}

/// 每次 `connect` 都启动 fresh child 的 opaque source。
#[derive(Clone)]
pub struct StdioAgentKernelSource {
    options: StdioAgentKernelTransportOptions,
}

impl StdioAgentKernelSource {
    /// 创建每次 connect 都启动 fresh child 的 source。
    pub fn new(options: StdioAgentKernelTransportOptions) -> Self {
        Self { options }
    }
}

impl fmt::Debug for StdioAgentKernelSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioAgentKernelSource")
            .field("options", &self.options)
            .finish()
    }
}

impl AgentKernelSource for StdioAgentKernelSource {
    fn connect(&self) -> Result<AgentKernelConnection, AgentKernelConnectError> {
        let (transport, events) = StdioAgentKernelTransport::spawn(self.options.clone())
            .map_err(|_| AgentKernelConnectError::Unavailable)?;
        Ok(AgentKernelConnection::new(transport, events))
    }
}

/// stdio child construction 的 closed、脱敏错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StdioAgentKernelTransportError {
    #[error("Agent kernel stdio options are invalid")]
    InvalidOptions,
    #[error("Agent kernel child process could not be spawned")]
    Spawn,
    #[error("Agent kernel child process pipes are unavailable")]
    Pipes,
    #[error("Agent kernel transport worker could not be started")]
    Worker,
}

struct StdioAgentKernelTransport {
    inner: Arc<StdioTransportInner>,
}

impl fmt::Debug for StdioAgentKernelTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioAgentKernelTransport")
            .field("is_closed", &self.inner.closed.load(Ordering::Acquire))
            .field("pending_count", &lock(&self.inner.pending).len())
            .finish()
    }
}

struct StdioTransportInner {
    closed: AtomicBool,
    shutdown: Mutex<ShutdownState>,
    shutdown_complete: Condvar,
    close_error: Mutex<Option<AgentKernelTransportError>>,
    codec: FrameCodec,
    request_sender: Mutex<Option<SyncSender<OutboundRequest>>>,
    event_sender: Mutex<Option<AgentKernelEventSink>>,
    pending: Mutex<
        BTreeMap<String, mpsc::Sender<Result<AgentKernelResponse, AgentKernelTransportError>>>,
    >,
    late_request_ids: Mutex<LateRequestIds>,
    child: Mutex<Option<Child>>,
    joins: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Default)]
struct ShutdownState {
    is_running: bool,
    result: Option<Result<(), AgentKernelTransportError>>,
}

#[derive(Default)]
struct LateRequestIds {
    ids: VecDeque<String>,
}

impl LateRequestIds {
    fn remember(&mut self, request_id: String) {
        if let Some(index) = self.ids.iter().position(|id| id == &request_id) {
            self.ids.remove(index);
        }
        self.ids.push_back(request_id);
        while self.ids.len() > LATE_REQUEST_TOMBSTONE_CAPACITY {
            self.ids.pop_front();
        }
    }

    fn take(&mut self, request_id: &str) -> bool {
        let Some(index) = self.ids.iter().position(|id| id == request_id) else {
            return false;
        };
        self.ids.remove(index);
        true
    }

    fn contains(&self, request_id: &str) -> bool {
        self.ids.iter().any(|id| id == request_id)
    }
}

struct OutboundRequest {
    request: AgentKernelRequest,
}

struct PendingRequestGuard {
    inner: Arc<StdioTransportInner>,
    request_id: String,
    is_active: bool,
}

impl StdioAgentKernelTransport {
    fn spawn(
        options: StdioAgentKernelTransportOptions,
    ) -> Result<(Self, AgentKernelEventStream), StdioAgentKernelTransportError> {
        options.validate()?;
        let mut command = Command::new(&options.executable);
        command
            .args(&options.arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear()
            .envs(&options.environment);
        if let Some(working_directory) = &options.working_directory {
            command.current_dir(working_directory);
        }
        let mut child = command
            .spawn()
            .map_err(|_| StdioAgentKernelTransportError::Spawn)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioAgentKernelTransportError::Pipes
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioAgentKernelTransportError::Pipes
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioAgentKernelTransportError::Pipes
        })?;
        let (request_sender, request_receiver) = mpsc::sync_channel(options.queue_capacity);
        let event_capacity = std::num::NonZeroUsize::new(options.queue_capacity)
            .expect("validated queue capacity is non-zero");
        let (event_sender, event_receiver) = AgentKernelEventStream::bounded(event_capacity);
        let inner = Arc::new(StdioTransportInner {
            closed: AtomicBool::new(false),
            shutdown: Mutex::new(ShutdownState::default()),
            shutdown_complete: Condvar::new(),
            close_error: Mutex::new(None),
            codec: options.frame_codec,
            request_sender: Mutex::new(Some(request_sender)),
            event_sender: Mutex::new(Some(event_sender)),
            pending: Mutex::new(BTreeMap::new()),
            late_request_ids: Mutex::new(LateRequestIds::default()),
            child: Mutex::new(Some(child)),
            joins: Mutex::new(Vec::new()),
        });

        let writer = spawn_worker("hunea-agent-kernel-stdio-writer", {
            let inner = Arc::clone(&inner);
            move || writer_loop(inner, stdin, request_receiver)
        })
        .map_err(|_| {
            close_inner(&inner, AgentKernelTransportError::Unavailable);
            StdioAgentKernelTransportError::Worker
        })?;
        let reader = match spawn_worker("hunea-agent-kernel-stdio-reader", {
            let inner = Arc::clone(&inner);
            move || reader_loop(inner, stdout)
        }) {
            Ok(reader) => reader,
            Err(_) => {
                close_inner(&inner, AgentKernelTransportError::Unavailable);
                let _ = writer.join();
                return Err(StdioAgentKernelTransportError::Worker);
            }
        };
        let stderr_reader = match spawn_worker("hunea-agent-kernel-stdio-stderr", move || {
            stderr_loop(stderr);
        }) {
            Ok(stderr_reader) => stderr_reader,
            Err(_) => {
                close_inner(&inner, AgentKernelTransportError::Unavailable);
                let _ = writer.join();
                let _ = reader.join();
                return Err(StdioAgentKernelTransportError::Worker);
            }
        };
        lock(&inner.joins).extend([writer, reader, stderr_reader]);
        Ok((Self { inner }, event_receiver))
    }
}

impl AgentKernelRequestTransport for StdioAgentKernelTransport {
    fn request(
        &self,
        request: AgentKernelRequest,
    ) -> Result<AgentKernelResponse, AgentKernelTransportError> {
        request
            .validate()
            .map_err(|_| AgentKernelTransportError::Protocol)?;
        let request_id = request.request_id().to_string();
        let deadline = request
            .deadline_ms()
            .ok_or(AgentKernelTransportError::Protocol)?;
        let (sender, receiver) = mpsc::channel();
        {
            let request_sender = lock(&self.inner.request_sender);
            let mut pending = lock(&self.inner.pending);
            if self.inner.closed.load(Ordering::Acquire) {
                return Err(self.inner.closed_error());
            }
            if pending.contains_key(&request_id)
                || lock(&self.inner.late_request_ids).contains(&request_id)
            {
                return Err(AgentKernelTransportError::Protocol);
            }
            pending.insert(request_id.clone(), sender);
            let queued = request_sender
                .as_ref()
                .is_some_and(|sender| sender.try_send(OutboundRequest { request }).is_ok());
            if !queued {
                pending.remove(&request_id);
                return Err(AgentKernelTransportError::Unavailable);
            }
        }
        let mut guard = PendingRequestGuard {
            inner: Arc::clone(&self.inner),
            request_id,
            is_active: true,
        };
        match receiver.recv_timeout(Duration::from_millis(deadline)) {
            Ok(response) => {
                guard.is_active = false;
                response
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Err(AgentKernelTransportError::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self.inner.closed_error()),
        }
    }

    fn shutdown(&self) -> Result<(), AgentKernelTransportError> {
        shutdown_inner(&self.inner)
    }
}

impl Drop for StdioAgentKernelTransport {
    fn drop(&mut self) {
        let _ = shutdown_inner(&self.inner);
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        if !self.is_active {
            return;
        }
        if lock(&self.inner.pending).remove(&self.request_id).is_some() {
            lock(&self.inner.late_request_ids).remember(self.request_id.clone());
        }
    }
}

impl StdioTransportInner {
    fn closed_error(&self) -> AgentKernelTransportError {
        lock(&self.close_error).unwrap_or(AgentKernelTransportError::ShutDown)
    }

    fn fail_all(&self, error: AgentKernelTransportError) {
        let pending = std::mem::take(&mut *lock(&self.pending));
        for (_, sender) in pending {
            let _ = sender.send(Err(error));
        }
    }
}

fn close_inner(inner: &Arc<StdioTransportInner>, error: AgentKernelTransportError) {
    if inner.closed.swap(true, Ordering::AcqRel) {
        return;
    }
    *lock(&inner.close_error) = Some(error);
    lock(&inner.request_sender).take();
    lock(&inner.event_sender).take();
    inner.fail_all(error);
    if let Some(mut child) = lock(&inner.child).take() {
        terminate_child(&mut child);
    }
}

fn shutdown_inner(inner: &Arc<StdioTransportInner>) -> Result<(), AgentKernelTransportError> {
    {
        let mut shutdown = lock(&inner.shutdown);
        loop {
            if let Some(result) = shutdown.result {
                return result;
            }
            if !shutdown.is_running {
                shutdown.is_running = true;
                break;
            }
            shutdown = inner
                .shutdown_complete
                .wait(shutdown)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    close_inner(inner, AgentKernelTransportError::ShutDown);
    let current = thread::current().id();
    let joins = std::mem::take(&mut *lock(&inner.joins));
    let mut failed = false;
    for join in joins {
        if join.thread().id() != current && join.join().is_err() {
            failed = true;
        }
    }
    let result = if failed {
        Err(AgentKernelTransportError::Unavailable)
    } else {
        Ok(())
    };
    let mut shutdown = lock(&inner.shutdown);
    shutdown.result = Some(result);
    inner.shutdown_complete.notify_all();
    result
}

fn spawn_worker(
    name: &'static str,
    worker: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.to_string()).spawn(worker)
}

fn writer_loop(
    inner: Arc<StdioTransportInner>,
    mut stdin: ChildStdin,
    receiver: Receiver<OutboundRequest>,
) {
    while let Ok(outbound) = receiver.recv() {
        if inner.closed.load(Ordering::Acquire) {
            break;
        }
        if inner
            .codec
            .write_json(&mut stdin, &outbound.request)
            .is_err()
        {
            close_inner(&inner, AgentKernelTransportError::Unavailable);
            break;
        }
    }
}

fn reader_loop(inner: Arc<StdioTransportInner>, mut stdout: ChildStdout) {
    loop {
        match inner.codec.read_json::<_, AgentKernelMessage>(&mut stdout) {
            Ok(message) => {
                if inner.closed.load(Ordering::Acquire) {
                    break;
                }
                if message.validate().is_err() {
                    close_inner(&inner, AgentKernelTransportError::Protocol);
                    break;
                }
                match message {
                    AgentKernelMessage::Response { response } => {
                        let sender = {
                            let mut pending = lock(&inner.pending);
                            if let Some(sender) = pending.remove(response.request_id()) {
                                Some(sender)
                            } else if lock(&inner.late_request_ids).take(response.request_id()) {
                                None
                            } else {
                                drop(pending);
                                close_inner(&inner, AgentKernelTransportError::Protocol);
                                break;
                            }
                        };
                        if let Some(sender) = sender {
                            let _ = sender.send(Ok(response));
                        }
                    }
                    AgentKernelMessage::Event { event } => {
                        let queued = lock(&inner.event_sender)
                            .as_ref()
                            .is_some_and(|sender| sender.try_send(event).is_ok());
                        if !queued {
                            close_inner(&inner, AgentKernelTransportError::Protocol);
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                let error = match error {
                    FrameError::Io | FrameError::TruncatedHeader | FrameError::TruncatedBody => {
                        AgentKernelTransportError::Unavailable
                    }
                    _ => AgentKernelTransportError::Protocol,
                };
                close_inner(&inner, error);
                break;
            }
        }
    }
}

fn stderr_loop(mut stderr: ChildStderr) {
    let mut buffer = [0_u8; 1024];
    while stderr.read(&mut buffer).is_ok_and(|count| count > 0) {}
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_debug_redacts_process_values() {
        let options = StdioAgentKernelTransportOptions::new("/secret/executable")
            .arg("secret-argument")
            .working_directory("/secret/cwd")
            .environment("SECRET_KEY", "secret-value");
        let debug = format!("{options:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("argument_count"));
    }

    #[test]
    fn shutdown_caches_worker_join_failure() {
        let (request_sender, _request_receiver) = mpsc::sync_channel(1);
        let (event_sender, _event_stream) = AgentKernelEventStream::bounded(
            std::num::NonZeroUsize::new(1).expect("literal capacity is non-zero"),
        );
        let failed_worker = thread::spawn(|| panic!("worker failure sentinel"));
        let inner = Arc::new(StdioTransportInner {
            closed: AtomicBool::new(false),
            shutdown: Mutex::new(ShutdownState::default()),
            shutdown_complete: Condvar::new(),
            close_error: Mutex::new(None),
            codec: FrameCodec::default(),
            request_sender: Mutex::new(Some(request_sender)),
            event_sender: Mutex::new(Some(event_sender)),
            pending: Mutex::new(BTreeMap::new()),
            late_request_ids: Mutex::new(LateRequestIds::default()),
            child: Mutex::new(None),
            joins: Mutex::new(vec![failed_worker]),
        });

        assert_eq!(
            shutdown_inner(&inner),
            Err(AgentKernelTransportError::Unavailable)
        );
        assert_eq!(
            shutdown_inner(&inner),
            Err(AgentKernelTransportError::Unavailable)
        );
    }
}
