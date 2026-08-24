//! 使用 dedicated blocking workers 管理 child process 的 stdio transport。

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt,
    io::Read,
    path::PathBuf,
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
};

use extension_protocol::{ExtensionRequest, ExtensionResponse};
use stdio_framing::{FrameCodec, FrameError};
use tokio::sync::oneshot;

use super::{
    ExtensionBundle, ExtensionBundleSource, ExtensionClient, ExtensionDiscoveryError,
    ExtensionOptions, ExtensionRequestFuture, ExtensionRequestTransport, ExtensionTransportError,
};

const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// child process launch 的 host-owned typed options。
#[derive(Clone)]
pub struct StdioTransportOptions {
    executable: PathBuf,
    arguments: Vec<OsString>,
    working_directory: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    frame_codec: FrameCodec,
    queue_capacity: usize,
}

impl fmt::Debug for StdioTransportOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioTransportOptions")
            .field("has_executable", &!self.executable.as_os_str().is_empty())
            .field("argument_count", &self.arguments.len())
            .field("has_working_directory", &self.working_directory.is_some())
            .field("environment_count", &self.environment.len())
            .field("max_frame_bytes", &self.frame_codec.max_frame_bytes())
            .field("queue_capacity", &self.queue_capacity)
            .finish()
    }
}

impl StdioTransportOptions {
    /// 创建只使用显式 executable 的 launch options。
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

    /// 追加一个不会进入 diagnostics 的 argv 值。
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.arguments.push(argument.into());
        self
    }

    /// 设置显式 working directory。
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());
        self
    }

    /// 只设置 host 明确允许传入 child 的 environment 项。
    pub fn environment(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// 设置 protocol frame 上限。
    pub fn max_frame_bytes(mut self, max_frame_bytes: usize) -> Result<Self, StdioTransportError> {
        self.frame_codec =
            FrameCodec::new(max_frame_bytes).map_err(|_| StdioTransportError::InvalidOptions)?;
        Ok(self)
    }

    /// 设置 writer queue 上限。
    pub fn queue_capacity(mut self, queue_capacity: usize) -> Result<Self, StdioTransportError> {
        if queue_capacity == 0 {
            return Err(StdioTransportError::InvalidOptions);
        }
        self.queue_capacity = queue_capacity;
        Ok(self)
    }

    fn validate(&self) -> Result<(), StdioTransportError> {
        if self.executable.as_os_str().is_empty() || self.queue_capacity == 0 {
            return Err(StdioTransportError::InvalidOptions);
        }
        Ok(())
    }
}

/// 可在 component dependency generation 恢复时重新启动 stdio child 并完成 discovery 的 source。
#[derive(Clone)]
pub struct StdioExtensionSource {
    transport_options: StdioTransportOptions,
    extension_options: ExtensionOptions,
}

impl fmt::Debug for StdioExtensionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioExtensionSource")
            .field("transport_options", &self.transport_options)
            .field("extension_options", &self.extension_options)
            .finish()
    }
}

impl StdioExtensionSource {
    /// 创建由 host 完全控制 launch 与 extension policy 的 rediscovery source。
    pub fn new(
        transport_options: StdioTransportOptions,
        extension_options: ExtensionOptions,
    ) -> Self {
        Self {
            transport_options,
            extension_options,
        }
    }

    /// 在 dedicated thread/runtime 中完成一次 blocking bridge，避免在现有 async runtime 中嵌套
    /// `block_on`；返回的 bundle 会携带 source 以支持后续 dependency restore。
    pub fn discover(&self) -> Result<ExtensionBundle, ExtensionDiscoveryError> {
        let transport_options = self.transport_options.clone();
        let extension_options = self.extension_options;
        let source = Arc::new(self.clone()) as Arc<dyn ExtensionBundleSource>;
        let join = thread::Builder::new()
            .name("hunea-extension-stdio-discovery".to_string())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| {
                        ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable)
                    })?;
                runtime.block_on(async move {
                    let transport =
                        StdioExtensionTransport::spawn(transport_options).map_err(|_| {
                            ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable)
                        })?;
                    ExtensionClient::new(transport, extension_options)
                        .discover()
                        .await
                        .map(|bundle| bundle.with_rediscovery_source(source))
                })
            })
            .map_err(|_| {
                ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable)
            })?;
        join.join()
            .map_err(|_| ExtensionDiscoveryError::Transport(ExtensionTransportError::Unavailable))?
    }
}

impl ExtensionBundleSource for StdioExtensionSource {
    fn discover(&self) -> Result<ExtensionBundle, ExtensionDiscoveryError> {
        StdioExtensionSource::discover(self)
    }
}

/// child spawn、pipe 或 worker 初始化失败时的 closed error projection。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StdioTransportError {
    #[error("stdio transport options are invalid")]
    InvalidOptions,
    #[error("extension child process could not be spawned")]
    Spawn,
    #[error("extension child process pipes are unavailable")]
    Pipes,
    #[error("extension transport worker could not be started")]
    Worker,
}

/// 由专用 blocking worker 拥有 child pipe 的 transport。
pub struct StdioExtensionTransport {
    inner: Arc<StdioTransportInner>,
}

impl fmt::Debug for StdioExtensionTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioExtensionTransport")
            .field("is_closed", &self.inner.closed.load(Ordering::Acquire))
            .field("pending_count", &pending_count(&self.inner))
            .finish()
    }
}

struct StdioTransportInner {
    closed: AtomicBool,
    close_error: Mutex<Option<ExtensionTransportError>>,
    shutdown_started: AtomicBool,
    external_handles: AtomicUsize,
    codec: FrameCodec,
    request_sender: Mutex<Option<SyncSender<OutboundRequest>>>,
    pending: Mutex<
        BTreeMap<String, oneshot::Sender<Result<ExtensionResponse, ExtensionTransportError>>>,
    >,
    late_request_ids: Mutex<LateRequestIds>,
    child: Mutex<Option<Child>>,
    joins: Mutex<Vec<JoinHandle<()>>>,
}

const LATE_REQUEST_TOMBSTONE_CAPACITY: usize = 64;

#[derive(Default)]
struct LateRequestIds {
    ids: std::collections::VecDeque<String>,
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
    request: ExtensionRequest,
}

struct PendingRequestGuard {
    inner: Arc<StdioTransportInner>,
    request_id: String,
    is_active: bool,
}

impl StdioExtensionTransport {
    /// 启动 child 与 stdio workers；不会读取配置或自动发现 executable。
    pub fn spawn(options: StdioTransportOptions) -> Result<Self, StdioTransportError> {
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
        let mut child = command.spawn().map_err(|_| StdioTransportError::Spawn)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioTransportError::Pipes
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioTransportError::Pipes
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            terminate_child(&mut child);
            StdioTransportError::Pipes
        })?;

        let (request_sender, request_receiver) = mpsc::sync_channel(options.queue_capacity);
        let inner = Arc::new(StdioTransportInner {
            closed: AtomicBool::new(false),
            close_error: Mutex::new(None),
            shutdown_started: AtomicBool::new(false),
            external_handles: AtomicUsize::new(1),
            codec: options.frame_codec,
            request_sender: Mutex::new(Some(request_sender)),
            pending: Mutex::new(BTreeMap::new()),
            late_request_ids: Mutex::new(LateRequestIds::default()),
            child: Mutex::new(Some(child)),
            joins: Mutex::new(Vec::new()),
        });

        let writer = spawn_worker("hunea-extension-stdio-writer", {
            let inner = Arc::clone(&inner);
            move || writer_loop(inner, stdin, request_receiver)
        })
        .map_err(|_| {
            let _ = shutdown_inner(&inner);
            StdioTransportError::Worker
        })?;
        let reader = match spawn_worker("hunea-extension-stdio-reader", {
            let inner = Arc::clone(&inner);
            move || reader_loop(inner, stdout)
        }) {
            Ok(reader) => reader,
            Err(_) => {
                let _ = shutdown_inner(&inner);
                let _ = writer.join();
                return Err(StdioTransportError::Worker);
            }
        };
        let stderr_reader =
            match spawn_worker("hunea-extension-stdio-stderr", move || stderr_loop(stderr)) {
                Ok(stderr_reader) => stderr_reader,
                Err(_) => {
                    let _ = shutdown_inner(&inner);
                    let _ = writer.join();
                    let _ = reader.join();
                    return Err(StdioTransportError::Worker);
                }
            };
        inner
            .joins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend([writer, reader, stderr_reader]);
        Ok(Self { inner })
    }

    /// 关闭 request admission、pending future、workers 与 child；重复调用安全。
    pub fn shutdown(&self) -> Result<(), ExtensionTransportError> {
        shutdown_inner(&self.inner)
    }
}

impl Clone for StdioExtensionTransport {
    fn clone(&self) -> Self {
        self.inner.external_handles.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl ExtensionRequestTransport for StdioExtensionTransport {
    fn request(&self, request: ExtensionRequest) -> ExtensionRequestFuture<'_> {
        let request_id = request.request_id().to_string();
        let (sender, receiver) = oneshot::channel();
        let admission = {
            // sender -> pending 的锁顺序与 shutdown/mark_closed 一致，避免 request
            // 已登记但尚未入队时被关闭路径遗留为悬挂 future。
            let request_sender = self
                .inner
                .request_sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.inner.closed.load(Ordering::Acquire) {
                Err(self.inner.closed_error())
            } else if pending.contains_key(&request_id)
                || self
                    .inner
                    .late_request_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&request_id)
            {
                Err(ExtensionTransportError::Protocol)
            } else {
                pending.insert(request_id.clone(), sender);
                let queued = request_sender
                    .as_ref()
                    .is_some_and(|sender| sender.try_send(OutboundRequest { request }).is_ok());
                if queued {
                    Ok(())
                } else {
                    pending.remove(&request_id);
                    Err(ExtensionTransportError::Unavailable)
                }
            }
        };
        if let Err(error) = admission {
            return Box::pin(async move { Err(error) });
        }

        let pending_guard = PendingRequestGuard {
            inner: Arc::clone(&self.inner),
            request_id,
            is_active: true,
        };
        Box::pin(await_pending_response(receiver, pending_guard))
    }

    fn shutdown(&self) -> Result<(), ExtensionTransportError> {
        StdioExtensionTransport::shutdown(self)
    }
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        if self.is_active {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending.remove(&self.request_id).is_some() {
                self.inner
                    .late_request_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remember(self.request_id.clone());
            }
        }
    }
}

impl PendingRequestGuard {
    fn complete(&mut self) {
        self.is_active = false;
    }
}

async fn await_pending_response(
    receiver: oneshot::Receiver<Result<ExtensionResponse, ExtensionTransportError>>,
    mut pending_guard: PendingRequestGuard,
) -> Result<ExtensionResponse, ExtensionTransportError> {
    let response = receiver
        .await
        .unwrap_or(Err(ExtensionTransportError::Unavailable));
    pending_guard.complete();
    response
}

impl Drop for StdioExtensionTransport {
    fn drop(&mut self) {
        if self.inner.external_handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _ = shutdown_inner(&self.inner);
        }
    }
}

impl StdioTransportInner {
    fn fail_all(&self, error: ExtensionTransportError) {
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for sender in pending.into_values() {
            let _ = sender.send(Err(error));
        }
    }

    fn mark_closed(&self, error: ExtensionTransportError) {
        self.closed.store(true, Ordering::Release);
        self.set_close_error(error);
        self.request_sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        self.fail_all(error);
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            terminate_child(&mut child);
        }
    }

    fn set_close_error(&self, error: ExtensionTransportError) {
        let mut close_error = self
            .close_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if close_error.is_none() {
            *close_error = Some(error);
        }
    }

    fn closed_error(&self) -> ExtensionTransportError {
        self.close_error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .unwrap_or(ExtensionTransportError::ShutDown)
    }
}

fn pending_count(inner: &StdioTransportInner) -> usize {
    inner
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len()
}

fn shutdown_inner(inner: &Arc<StdioTransportInner>) -> Result<(), ExtensionTransportError> {
    if inner.shutdown_started.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    inner.closed.store(true, Ordering::Release);
    inner.set_close_error(ExtensionTransportError::ShutDown);
    inner
        .request_sender
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    inner.fail_all(ExtensionTransportError::ShutDown);
    if let Some(mut child) = inner
        .child
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        terminate_child(&mut child);
    }
    let current = thread::current().id();
    let joins = std::mem::take(
        &mut *inner
            .joins
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for join in joins {
        if join.thread().id() != current {
            let _ = join.join();
        }
    }
    Ok(())
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
    receiver: mpsc::Receiver<OutboundRequest>,
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
            inner.mark_closed(ExtensionTransportError::Unavailable);
            break;
        }
    }
}

fn reader_loop(inner: Arc<StdioTransportInner>, mut stdout: ChildStdout) {
    loop {
        match inner.codec.read_json::<_, ExtensionResponse>(&mut stdout) {
            Ok(response) => {
                if inner.closed.load(Ordering::Acquire) {
                    break;
                }
                let correlation = {
                    let mut pending = inner
                        .pending
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(sender) = pending.remove(response.request_id()) {
                        Some(sender)
                    } else if inner
                        .late_request_ids
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take(response.request_id())
                    {
                        None
                    } else {
                        drop(pending);
                        inner.mark_closed(ExtensionTransportError::Protocol);
                        break;
                    }
                };
                if let Some(sender) = correlation {
                    let _ = sender.send(Ok(response));
                }
            }
            Err(error) => {
                let transport_error = match error {
                    FrameError::Io | FrameError::TruncatedHeader | FrameError::TruncatedBody => {
                        ExtensionTransportError::Unavailable
                    }
                    _ => ExtensionTransportError::Protocol,
                };
                inner.mark_closed(transport_error);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_options_debug_redacts_process_values() {
        let options = StdioTransportOptions::new("/secret/executable")
            .arg("secret-argument")
            .working_directory("/secret/cwd")
            .environment("SECRET_KEY", "secret-value");
        let debug = format!("{options:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("argument_count"));
    }
}
