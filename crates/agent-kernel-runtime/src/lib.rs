//! 进程外 Agent kernel 的 transport-neutral `AgentRuntime` adapter。

mod codec;
mod stdio;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    num::{NonZeroU64, NonZeroUsize},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use agent_kernel_protocol::{
    AgentKernelCapability, AgentKernelCommandParams, AgentKernelCommandReceipt,
    AgentKernelCommandResult, AgentKernelEventNotification, AgentKernelInitializeParams,
    AgentKernelInitializeResult, AgentKernelMethod, AgentKernelRequest, AgentKernelResponse,
    AgentKernelShutdownParams, AgentKernelShutdownResult,
};
use runtime_domain::{
    agent::{
        AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentRuntime,
        AgentRuntimeError, AgentTurnId,
    },
    event_notifier::RuntimeEventNotifier,
    session::RuntimeTarget,
};
use thiserror::Error;

pub use stdio::{
    StdioAgentKernelSource, StdioAgentKernelTransportError, StdioAgentKernelTransportOptions,
};

const EVENT_BRIDGE_POLL: Duration = Duration::from_millis(20);
const MAX_BUFFERED_AGENT_EVENTS: usize = 64;
const TRANSPORT_FAILURE_TEXT: &str = "External Agent kernel transport failed";
const PROTOCOL_FAILURE_TEXT: &str = "External Agent kernel protocol exchange was invalid";

/// Kernel request transport；implementation 必须遵守 request envelope 的 deadline。
pub trait AgentKernelRequestTransport: Send + Sync {
    /// 发送一个已校验 request，并等待 correlated response。
    fn request(
        &self,
        request: AgentKernelRequest,
    ) -> Result<AgentKernelResponse, AgentKernelTransportError>;

    /// 立即关闭 fresh admission、唤醒 event receiver 并撤销全部 transport effects。
    fn shutdown(&self) -> Result<(), AgentKernelTransportError>;
}

/// Bounded unsolicited-event producer；不会暴露 channel payload diagnostics。
#[derive(Clone)]
pub struct AgentKernelEventSink {
    sender: SyncSender<AgentKernelEventNotification>,
}

impl AgentKernelEventSink {
    /// 阻塞到 event 进入 bounded queue，或 receiver 已关闭。
    pub fn send(
        &self,
        event: AgentKernelEventNotification,
    ) -> Result<(), AgentKernelEventSendError> {
        self.sender
            .send(event)
            .map_err(|_| AgentKernelEventSendError::Closed)
    }

    /// 尝试把 event 放入 bounded queue；queue 满时显式返回 saturation。
    pub fn try_send(
        &self,
        event: AgentKernelEventNotification,
    ) -> Result<(), AgentKernelEventSendError> {
        self.sender.try_send(event).map_err(|error| match error {
            TrySendError::Full(_) => AgentKernelEventSendError::Saturated,
            TrySendError::Disconnected(_) => AgentKernelEventSendError::Closed,
        })
    }
}

impl fmt::Debug for AgentKernelEventSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentKernelEventSink")
    }
}

/// Event producer 观察到的 closed queue 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentKernelEventSendError {
    #[error("Agent kernel event stream is saturated")]
    Saturated,
    #[error("Agent kernel event stream is closed")]
    Closed,
}

/// 只能通过 [`AgentKernelEventStream::bounded`] 创建的 bounded event stream。
pub struct AgentKernelEventStream {
    receiver: Receiver<AgentKernelEventNotification>,
}

impl AgentKernelEventStream {
    /// 创建具有明确非零容量的 event sink/stream pair。
    pub fn bounded(capacity: NonZeroUsize) -> (AgentKernelEventSink, Self) {
        let (sender, receiver) = mpsc::sync_channel(capacity.get());
        (AgentKernelEventSink { sender }, Self { receiver })
    }

    fn try_recv(&self) -> Result<AgentKernelEventNotification, std::sync::mpsc::TryRecvError> {
        self.receiver.try_recv()
    }

    fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<AgentKernelEventNotification, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
}

impl fmt::Debug for AgentKernelEventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentKernelEventStream")
    }
}

/// 一个 fresh transport owner 与唯一 unsolicited-event receiver。
pub struct AgentKernelConnection {
    transport: Arc<dyn AgentKernelRequestTransport>,
    events: AgentKernelEventStream,
}

impl AgentKernelConnection {
    /// 组合 request transport 与同 generation 的 bounded event stream。
    pub fn new<T>(transport: T, events: AgentKernelEventStream) -> Self
    where
        T: AgentKernelRequestTransport + 'static,
    {
        Self {
            transport: Arc::new(transport),
            events,
        }
    }
}

impl fmt::Debug for AgentKernelConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentKernelConnection")
    }
}

/// 每次 activation 都必须返回 fresh connection 的显式 construction authority。
pub trait AgentKernelSource: Send + Sync {
    /// 建立一个不可复用的 request transport 与 bounded event stream generation。
    fn connect(&self) -> Result<AgentKernelConnection, AgentKernelConnectError>;
}

/// Source 无法建立 fresh connection 时的脱敏错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentKernelConnectError {
    #[error("Agent kernel connection is unavailable")]
    Unavailable,
}

/// Transport boundary 的 closed error projection。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AgentKernelTransportError {
    #[error("Agent kernel transport is unavailable")]
    Unavailable,
    #[error("Agent kernel transport is shut down")]
    ShutDown,
    #[error("Agent kernel transport rejected the protocol exchange")]
    Protocol,
    #[error("Agent kernel transport request timed out")]
    Timeout,
}

/// External Agent generation activation 的 closed、脱敏错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExternalAgentActivationError {
    #[error("External Agent adapter is finalized")]
    Finalized,
    #[error("External Agent adapter is already active")]
    AlreadyActive,
    #[error("External Agent activation cleanup is still pending")]
    CleanupPending,
    #[error("External Agent kernel connection is unavailable")]
    ConnectionUnavailable,
    #[error("External Agent kernel initialization failed")]
    InitializationTransport,
    #[error("External Agent kernel initialization is invalid")]
    InvalidInitialization,
    #[error("External Agent kernel rejected initialization")]
    InitializationRejected,
    #[error("External Agent kernel capability negotiation failed")]
    CapabilityNegotiation,
    #[error("External Agent kernel sent an event before a command")]
    EventBeforeCommand,
    #[error("External Agent kernel event stream is unavailable")]
    EventStreamUnavailable,
    #[error("External Agent kernel event bridge could not be started")]
    EventBridgeUnavailable,
    #[error("External Agent activation cleanup failed")]
    Cleanup,
}

/// Host-owned request/shutdown deadlines。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalAgentRuntimeOptions {
    request_deadline_ms: NonZeroU64,
    shutdown_grace_ms: NonZeroU64,
}

impl ExternalAgentRuntimeOptions {
    /// 创建 host-owned request 与 shutdown deadline。
    pub const fn new(request_deadline_ms: NonZeroU64, shutdown_grace_ms: NonZeroU64) -> Self {
        Self {
            request_deadline_ms,
            shutdown_grace_ms,
        }
    }

    /// 普通 request deadline，单位为毫秒。
    pub const fn request_deadline_ms(self) -> NonZeroU64 {
        self.request_deadline_ms
    }

    /// cooperative shutdown grace，单位为毫秒。
    pub const fn shutdown_grace_ms(self) -> NonZeroU64 {
        self.shutdown_grace_ms
    }
}

impl Default for ExternalAgentRuntimeOptions {
    fn default() -> Self {
        Self::new(
            NonZeroU64::new(30_000).expect("literal deadline is non-zero"),
            NonZeroU64::new(1_000).expect("literal grace is non-zero"),
        )
    }
}

/// Host-prepared external Agent adapter；construction 本身不创建 thread/process。
pub struct ExternalAgentRuntime {
    source: Arc<dyn AgentKernelSource>,
    options: ExternalAgentRuntimeOptions,
    generation: Option<ActiveGeneration>,
    activation_cleanup: Option<Arc<dyn AgentKernelRequestTransport>>,
    finalization: FinalizationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinalizationState {
    Open,
    Finalizing,
    Succeeded,
}

impl fmt::Debug for ExternalAgentRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExternalAgentRuntime")
            .field("is_active", &self.generation.is_some())
            .field("has_activation_cleanup", &self.activation_cleanup.is_some())
            .field(
                "is_finalized",
                &matches!(
                    self.finalization,
                    FinalizationState::Finalizing | FinalizationState::Succeeded
                ),
            )
            .field("options", &self.options)
            .finish()
    }
}

struct ActiveGeneration {
    transport: Arc<dyn AgentKernelRequestTransport>,
    shared: Arc<GenerationShared>,
    bridge: Option<JoinHandle<()>>,
    next_request_id: u64,
    next_command_id: u64,
    cleanup_started: bool,
}

struct GenerationShared {
    state: Mutex<GenerationState>,
    closing: AtomicBool,
    notifier: RuntimeEventNotifier,
}

#[derive(Default)]
struct GenerationState {
    last_sequence: u64,
    commands: BTreeMap<u64, TrackedCommand>,
    active_turn: Option<TurnIdentity>,
    pending_permissions: BTreeSet<String>,
    ready: VecDeque<AgentEvent>,
    is_failed: bool,
    terminal_seen: bool,
}

struct TrackedCommand {
    expectation: CommandExpectation,
    phase: CommandPhase,
    staged_permissions: BTreeSet<String>,
    staged_terminal: bool,
}

enum CommandPhase {
    Pending { events: Vec<AgentEvent> },
    Accepted,
}

#[derive(Clone)]
enum CommandExpectation {
    Submit(TurnIdentity),
    Interrupt {
        agent_id: AgentId,
        requested_target: Option<RuntimeTarget>,
        turn: Option<TurnIdentity>,
    },
    Permission {
        agent_id: AgentId,
        requested_target: Option<RuntimeTarget>,
        turn: Option<TurnIdentity>,
        request_id: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct TurnIdentity {
    agent_id: AgentId,
    turn_id: AgentTurnId,
    target: RuntimeTarget,
}

impl ExternalAgentRuntime {
    /// 创建 inactive adapter；source 直到 `activate` 才被调用。
    pub fn new(source: Arc<dyn AgentKernelSource>, options: ExternalAgentRuntimeOptions) -> Self {
        Self {
            source,
            options,
            generation: None,
            activation_cleanup: None,
            finalization: FinalizationState::Open,
        }
    }

    /// 建立 fresh generation 并绑定当前 event-stream capability generation。
    pub fn activate(
        &mut self,
        notifier: RuntimeEventNotifier,
    ) -> Result<(), ExternalAgentActivationError> {
        if self.finalization != FinalizationState::Open {
            return Err(ExternalAgentActivationError::Finalized);
        }
        if self.generation.is_some() {
            return Err(ExternalAgentActivationError::AlreadyActive);
        }
        if self.activation_cleanup.is_some() {
            return Err(ExternalAgentActivationError::CleanupPending);
        }

        let AgentKernelConnection { transport, events } = self
            .source
            .connect()
            .map_err(|_| ExternalAgentActivationError::ConnectionUnavailable)?;
        let mut guard = ConnectionGuard::new(Arc::clone(&transport));
        if let Err(error) = initialize_transport(&transport, self.options) {
            return Err(self.fail_activation(&mut guard, error));
        }
        match events.try_recv() {
            Ok(_) => {
                return Err(self.fail_activation(
                    &mut guard,
                    ExternalAgentActivationError::EventBeforeCommand,
                ));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(self.fail_activation(
                    &mut guard,
                    ExternalAgentActivationError::EventStreamUnavailable,
                ));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }

        let shared = Arc::new(GenerationShared {
            state: Mutex::new(GenerationState::default()),
            closing: AtomicBool::new(false),
            notifier,
        });
        let bridge = match spawn_event_bridge(Arc::clone(&shared), Arc::clone(&transport), events) {
            Ok(bridge) => bridge,
            Err(error) => return Err(self.fail_activation(&mut guard, error)),
        };
        guard.disarm();
        self.generation = Some(ActiveGeneration {
            transport,
            shared,
            bridge: Some(bridge),
            next_request_id: 2,
            next_command_id: 1,
            cleanup_started: false,
        });
        Ok(())
    }

    /// 完整撤销 active generation；重复调用安全。
    pub fn suspend(&mut self) -> Result<(), AgentRuntimeError> {
        if let Some(transport) = self.activation_cleanup.as_ref() {
            if transport.shutdown().is_err() {
                return Err(external_shutdown_error());
            }
            self.activation_cleanup = None;
        }
        let Some(generation) = self.generation.as_mut() else {
            return Ok(());
        };
        generation.shared.closing.store(true, Ordering::Release);

        if !generation.cleanup_started {
            generation.cleanup_started = true;
            let shutdown_request = AgentKernelRequest::new(
                generation.next_request_identity(),
                AgentKernelMethod::Shutdown,
                AgentKernelShutdownParams::default(),
            )
            .map(|request| request.with_deadline_ms(self.options.shutdown_grace_ms.get()));
            if let Ok(request) = shutdown_request
                && let Ok(response) = generation.transport.request(request)
            {
                let _ = response
                    .result::<AgentKernelShutdownResult>()
                    .map(|result| result.is_some_and(|result| result.drained));
            }
        }

        let shutdown = generation.transport.shutdown();
        let join = generation
            .bridge
            .take()
            .map(thread::JoinHandle::join)
            .transpose();
        generation.clear_state();
        if shutdown.is_err() || join.is_err() {
            return Err(external_shutdown_error());
        }
        self.generation = None;
        Ok(())
    }

    /// 返回当前 adapter 是否有未完成或尚未 drain 的 turn facts。
    pub fn is_busy(&self) -> bool {
        self.generation.as_ref().is_some_and(|generation| {
            let state = lock_state(&generation.shared.state);
            state.active_turn.is_some()
                || state
                    .commands
                    .values()
                    .any(|command| matches!(command.expectation, CommandExpectation::Submit(_)))
                || !state.ready.is_empty()
        })
    }

    fn fail_activation(
        &mut self,
        guard: &mut ConnectionGuard,
        error: ExternalAgentActivationError,
    ) -> ExternalAgentActivationError {
        let (error, cleanup) = guard.fail(error);
        self.activation_cleanup = cleanup;
        error
    }

    fn ensure_generation(&mut self) -> Result<&mut ActiveGeneration, AgentRuntimeError> {
        if self.finalization != FinalizationState::Open {
            return Err(AgentRuntimeError::Disposed);
        }
        let generation = self
            .generation
            .as_mut()
            .ok_or(AgentRuntimeError::Disposed)?;
        if generation.shared.closing.load(Ordering::Acquire) {
            return Err(AgentRuntimeError::Disposed);
        }
        Ok(generation)
    }

    fn dispatch_command(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        let request_deadline_ms = self.options.request_deadline_ms.get();
        let generation = self.ensure_generation()?;
        let (wire_command, expectation) = codec::encode_command(command)?;
        let command_id = generation.next_command_identity()?;
        {
            let mut state = lock_state(&generation.shared.state);
            state.admit_command(command_id, expectation)?;
        }
        let params = AgentKernelCommandParams {
            command_id,
            command: wire_command,
        };
        let request_id = generation.next_request_identity();
        let request =
            AgentKernelRequest::new(request_id.clone(), AgentKernelMethod::AgentCommand, params)
                .map_err(|_| AgentRuntimeError::CommandRejected(PROTOCOL_FAILURE_TEXT.to_string()))?
                .with_deadline_ms(request_deadline_ms);
        let response = generation.transport.request(request);
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                lock_state(&generation.shared.state).reject_command(command_id);
                return Err(map_transport_command_error(error));
            }
        };
        if response.validate().is_err() || response.request_id() != request_id {
            lock_state(&generation.shared.state).reject_command(command_id);
            return Err(AgentRuntimeError::CommandRejected(
                PROTOCOL_FAILURE_TEXT.to_string(),
            ));
        }
        if let Some(error) = response.error() {
            lock_state(&generation.shared.state).reject_command(command_id);
            return Err(codec::map_remote_error(error.code()));
        }
        let result = match response.result::<AgentKernelCommandResult>() {
            Ok(Some(result)) if result.validate().is_ok() => result,
            _ => {
                lock_state(&generation.shared.state).reject_command(command_id);
                return Err(AgentRuntimeError::CommandRejected(
                    PROTOCOL_FAILURE_TEXT.to_string(),
                ));
            }
        };
        if result.command_id != command_id {
            lock_state(&generation.shared.state).reject_command(command_id);
            return Err(AgentRuntimeError::CommandRejected(
                PROTOCOL_FAILURE_TEXT.to_string(),
            ));
        }
        let (receipt, should_wake) = {
            let mut state = lock_state(&generation.shared.state);
            if state.is_failed {
                return Err(AgentRuntimeError::CommandRejected(
                    TRANSPORT_FAILURE_TEXT.to_string(),
                ));
            }
            state.accept_command(command_id, result.receipt)?
        };
        if should_wake {
            generation.shared.notifier.notify();
        }
        Ok(receipt)
    }
}

impl AgentRuntime for ExternalAgentRuntime {
    fn dispatch(
        &mut self,
        command: AgentCommand,
    ) -> Result<AgentCommandReceipt, AgentRuntimeError> {
        self.dispatch_command(command)
    }

    fn drain_events(&mut self) -> Vec<AgentEvent> {
        let Some(generation) = self.generation.as_ref() else {
            return Vec::new();
        };
        let mut state = lock_state(&generation.shared.state);
        let events = state.ready.drain(..).collect::<Vec<_>>();
        if events.iter().any(|event| event.kind.is_terminal()) {
            state.active_turn = None;
            state.pending_permissions.clear();
            state.commands.clear();
            state.terminal_seen = false;
        }
        events
    }

    fn shutdown(&mut self) -> Result<(), AgentRuntimeError> {
        match self.finalization {
            FinalizationState::Succeeded => return Ok(()),
            FinalizationState::Open => self.finalization = FinalizationState::Finalizing,
            FinalizationState::Finalizing => {}
        }
        let result = self.suspend();
        if result.is_ok() {
            self.finalization = FinalizationState::Succeeded;
        }
        result
    }
}

impl Drop for ExternalAgentRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl ActiveGeneration {
    fn next_request_identity(&mut self) -> String {
        let current = self.next_request_id;
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .expect("Agent kernel request identity space should be unreachable");
        format!("agent-kernel-{current}")
    }

    fn next_command_identity(&mut self) -> Result<u64, AgentRuntimeError> {
        let current = self.next_command_id;
        self.next_command_id = self.next_command_id.checked_add(1).ok_or_else(|| {
            AgentRuntimeError::CommandRejected("Agent command identity is exhausted".to_string())
        })?;
        Ok(current)
    }

    fn clear_state(&self) {
        *lock_state(&self.shared.state) = GenerationState::default();
    }
}

impl GenerationState {
    fn admit_command(
        &mut self,
        command_id: u64,
        mut expectation: CommandExpectation,
    ) -> Result<(), AgentRuntimeError> {
        if self.is_failed {
            return Err(AgentRuntimeError::CommandRejected(
                TRANSPORT_FAILURE_TEXT.to_string(),
            ));
        }
        match &mut expectation {
            CommandExpectation::Submit(identity) => {
                if identity.agent_id != AgentId::MAIN {
                    return Err(AgentRuntimeError::UnknownAgent);
                }
                if self.active_turn.is_some()
                    || self
                        .commands
                        .values()
                        .any(|command| matches!(command.expectation, CommandExpectation::Submit(_)))
                {
                    return Err(AgentRuntimeError::Busy);
                }
            }
            CommandExpectation::Interrupt {
                agent_id,
                requested_target,
                turn,
            } => {
                ensure_agent(*agent_id)?;
                ensure_requested_target(self.active_turn.as_ref(), requested_target.as_ref())?;
                *turn = self.active_turn.clone();
            }
            CommandExpectation::Permission {
                agent_id,
                requested_target,
                turn,
                request_id,
            } => {
                ensure_agent(*agent_id)?;
                ensure_requested_target(self.active_turn.as_ref(), requested_target.as_ref())?;
                let active_turn = self.active_turn.clone().ok_or_else(|| {
                    AgentRuntimeError::CommandRejected(
                        "External Agent has no active turn".to_string(),
                    )
                })?;
                *turn = Some(active_turn);
                if !self.pending_permissions.contains(request_id) {
                    return Err(AgentRuntimeError::CommandRejected(
                        "External Agent has no matching permission request".to_string(),
                    ));
                }
            }
        }
        self.commands.insert(
            command_id,
            TrackedCommand {
                expectation,
                phase: CommandPhase::Pending { events: Vec::new() },
                staged_permissions: BTreeSet::new(),
                staged_terminal: false,
            },
        );
        Ok(())
    }

    fn reject_command(&mut self, command_id: u64) {
        self.commands.remove(&command_id);
    }

    fn accept_command(
        &mut self,
        command_id: u64,
        receipt: AgentKernelCommandReceipt,
    ) -> Result<(AgentCommandReceipt, bool), AgentRuntimeError> {
        let expectation = self
            .commands
            .get(&command_id)
            .map(|command| command.expectation.clone())
            .ok_or_else(|| AgentRuntimeError::CommandRejected(PROTOCOL_FAILURE_TEXT.to_string()))?;
        let host_receipt = match validate_receipt(&expectation, receipt) {
            Ok(receipt) => receipt,
            Err(error) => {
                self.reject_command(command_id);
                return Err(error);
            }
        };
        if self
            .commands
            .get(&command_id)
            .is_some_and(|command| matches!(command.phase, CommandPhase::Accepted))
        {
            self.reject_command(command_id);
            return Err(AgentRuntimeError::CommandRejected(
                PROTOCOL_FAILURE_TEXT.to_string(),
            ));
        }
        let command = self
            .commands
            .get_mut(&command_id)
            .expect("validated command must remain registered");
        let staged = match std::mem::replace(&mut command.phase, CommandPhase::Accepted) {
            CommandPhase::Pending { events } => events,
            CommandPhase::Accepted => unreachable!("accepted command was rejected above"),
        };
        match &command.expectation {
            CommandExpectation::Submit(turn) => self.active_turn = Some(turn.clone()),
            CommandExpectation::Permission { request_id, .. } => {
                self.pending_permissions.remove(request_id);
            }
            CommandExpectation::Interrupt { .. } => {}
        }
        self.pending_permissions
            .append(&mut command.staged_permissions);
        if command.staged_terminal {
            self.terminal_seen = true;
        }
        let should_wake = self.ready.is_empty() && !staged.is_empty();
        self.ready.extend(staged);
        Ok((host_receipt, should_wake))
    }

    fn accept_event(&mut self, notification: AgentKernelEventNotification) -> Result<bool, ()> {
        notification.validate().map_err(|_| ())?;
        let expected_sequence = self.last_sequence.checked_add(1).ok_or(())?;
        if notification.sequence != expected_sequence {
            return Err(());
        }
        if self.terminal_seen {
            return Err(());
        }
        let event = codec::decode_event(notification.event).map_err(|_| ())?;
        let command = self.commands.get(&notification.command_id).ok_or(())?;
        if !command.expectation.matches_event(&event) || command.staged_terminal {
            return Err(());
        }
        let buffered_event_count = self.ready.len()
            + self
                .commands
                .values()
                .map(|command| match &command.phase {
                    CommandPhase::Pending { events } => events.len(),
                    CommandPhase::Accepted => 0,
                })
                .sum::<usize>();
        if buffered_event_count >= MAX_BUFFERED_AGENT_EVENTS {
            return Err(());
        }
        let command = self
            .commands
            .get_mut(&notification.command_id)
            .expect("validated command must remain registered");
        if let AgentEventKind::PermissionRequested { request } = &event.kind
            && (!command
                .staged_permissions
                .insert(request.request_id.clone())
                || self.pending_permissions.contains(&request.request_id))
        {
            return Err(());
        }
        if event.kind.is_terminal() {
            command.staged_terminal = true;
        }
        self.last_sequence = notification.sequence;
        match &mut command.phase {
            CommandPhase::Pending { events } => {
                events.push(event);
                Ok(false)
            }
            CommandPhase::Accepted => {
                if let AgentEventKind::PermissionRequested { request } = &event.kind {
                    self.pending_permissions.insert(request.request_id.clone());
                }
                let should_wake = self.ready.is_empty();
                if event.kind.is_terminal() {
                    self.terminal_seen = true;
                }
                self.ready.push_back(event);
                Ok(should_wake)
            }
        }
    }

    fn fail(&mut self) -> bool {
        if self.is_failed {
            return false;
        }
        self.is_failed = true;
        self.commands.clear();
        self.pending_permissions.clear();
        let Some(turn) = self.active_turn.clone() else {
            return false;
        };
        let had_ready_event = !self.ready.is_empty();
        self.ready.retain(|event| !event.kind.is_terminal());
        let should_wake = self.ready.is_empty();
        self.ready.push_back(AgentEvent {
            agent_id: turn.agent_id,
            turn_id: turn.turn_id,
            target: turn.target,
            kind: AgentEventKind::TurnFailed {
                message: TRANSPORT_FAILURE_TEXT.to_string(),
            },
        });
        should_wake && !had_ready_event
    }
}

impl CommandExpectation {
    fn matches_event(&self, event: &AgentEvent) -> bool {
        let identity = match self {
            Self::Submit(identity) => Some(identity),
            Self::Permission { turn, .. } | Self::Interrupt { turn, .. } => turn.as_ref(),
        };
        identity.is_some_and(|identity| {
            event.agent_id == identity.agent_id
                && event.turn_id == identity.turn_id
                && event.target == identity.target
        })
    }
}

fn initialize_transport(
    transport: &Arc<dyn AgentKernelRequestTransport>,
    options: ExternalAgentRuntimeOptions,
) -> Result<(), ExternalAgentActivationError> {
    let required = vec![
        AgentKernelCapability::Events,
        AgentKernelCapability::Interrupt,
        AgentKernelCapability::PermissionResponse,
        AgentKernelCapability::StructuredErrors,
    ];
    let request_id = "agent-kernel-1";
    let request = AgentKernelRequest::new(
        request_id,
        AgentKernelMethod::Initialize,
        AgentKernelInitializeParams {
            protocol_version: agent_kernel_protocol::PROTOCOL_VERSION,
            capabilities: required.clone(),
            host_capabilities: Vec::new(),
        },
    )
    .map_err(|_| ExternalAgentActivationError::InvalidInitialization)?
    .with_deadline_ms(options.request_deadline_ms.get());
    let response = transport
        .request(request)
        .map_err(|_| ExternalAgentActivationError::InitializationTransport)?;
    response
        .validate()
        .map_err(|_| ExternalAgentActivationError::InvalidInitialization)?;
    if response.request_id() != request_id {
        return Err(ExternalAgentActivationError::InvalidInitialization);
    }
    if response.error().is_some() {
        return Err(ExternalAgentActivationError::InitializationRejected);
    }
    let result = response
        .result::<AgentKernelInitializeResult>()
        .map_err(|_| ExternalAgentActivationError::InvalidInitialization)?
        .ok_or(ExternalAgentActivationError::InvalidInitialization)?;
    result
        .validate()
        .map_err(|_| ExternalAgentActivationError::InvalidInitialization)?;
    if required
        .iter()
        .any(|capability| !result.capabilities.contains(capability))
        || !result.accepted_host_capabilities.is_empty()
    {
        return Err(ExternalAgentActivationError::CapabilityNegotiation);
    }
    Ok(())
}

fn spawn_event_bridge(
    shared: Arc<GenerationShared>,
    transport: Arc<dyn AgentKernelRequestTransport>,
    events: AgentKernelEventStream,
) -> Result<JoinHandle<()>, ExternalAgentActivationError> {
    thread::Builder::new()
        .name("hunea-agent-kernel-events".to_string())
        .spawn(move || {
            loop {
                if shared.closing.load(Ordering::Acquire) {
                    break;
                }
                match events.recv_timeout(EVENT_BRIDGE_POLL) {
                    Ok(event) => {
                        let accepted = lock_state(&shared.state).accept_event(event);
                        match accepted {
                            Ok(true) => shared.notifier.notify(),
                            Ok(false) => {}
                            Err(()) => {
                                shared.closing.store(true, Ordering::Release);
                                let should_wake = lock_state(&shared.state).fail();
                                let _ = transport.shutdown();
                                if should_wake {
                                    shared.notifier.notify();
                                }
                                break;
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => {
                        if !shared.closing.load(Ordering::Acquire) {
                            let should_wake = lock_state(&shared.state).fail();
                            let _ = transport.shutdown();
                            if should_wake {
                                shared.notifier.notify();
                            }
                        }
                        break;
                    }
                }
            }
        })
        .map_err(|_| ExternalAgentActivationError::EventBridgeUnavailable)
}

fn validate_receipt(
    expectation: &CommandExpectation,
    receipt: AgentKernelCommandReceipt,
) -> Result<AgentCommandReceipt, AgentRuntimeError> {
    match (expectation, receipt) {
        (
            CommandExpectation::Submit(expected),
            AgentKernelCommandReceipt::TurnStarted {
                turn_id,
                target,
                activity_label,
            },
        ) if turn_id == expected.turn_id.get()
            && codec::decode_target(target.clone()) == Ok(expected.target.clone()) =>
        {
            Ok(AgentCommandReceipt::TurnStarted {
                turn_id: expected.turn_id,
                target: expected.target.clone(),
                activity_label,
            })
        }
        (
            CommandExpectation::Interrupt { turn, .. },
            AgentKernelCommandReceipt::Interrupted { target },
        ) if codec::decode_optional_target(target.clone())
            == Ok(turn.as_ref().map(|turn| turn.target.clone())) =>
        {
            Ok(AgentCommandReceipt::Interrupted {
                target: turn.as_ref().map(|turn| turn.target.clone()),
            })
        }
        (CommandExpectation::Interrupt { .. }, AgentKernelCommandReceipt::Accepted)
        | (CommandExpectation::Permission { .. }, AgentKernelCommandReceipt::Accepted) => {
            Ok(AgentCommandReceipt::Accepted)
        }
        _ => Err(AgentRuntimeError::CommandRejected(
            PROTOCOL_FAILURE_TEXT.to_string(),
        )),
    }
}

fn ensure_agent(agent_id: AgentId) -> Result<(), AgentRuntimeError> {
    if agent_id != AgentId::MAIN {
        return Err(AgentRuntimeError::UnknownAgent);
    }
    Ok(())
}

fn ensure_requested_target(
    active: Option<&TurnIdentity>,
    requested: Option<&RuntimeTarget>,
) -> Result<(), AgentRuntimeError> {
    match (active, requested) {
        (None, None) => Ok(()),
        (Some(_), None) => Ok(()),
        (Some(active), Some(requested)) if &active.target == requested => Ok(()),
        _ => Err(AgentRuntimeError::CommandRejected(
            "External Agent target does not match the active turn".to_string(),
        )),
    }
}

fn map_transport_command_error(error: AgentKernelTransportError) -> AgentRuntimeError {
    let message = match error {
        AgentKernelTransportError::Timeout => "External Agent kernel command timed out",
        AgentKernelTransportError::Protocol => PROTOCOL_FAILURE_TEXT,
        AgentKernelTransportError::Unavailable | AgentKernelTransportError::ShutDown => {
            TRANSPORT_FAILURE_TEXT
        }
    };
    AgentRuntimeError::CommandRejected(message.to_string())
}

fn external_shutdown_error() -> AgentRuntimeError {
    AgentRuntimeError::Shutdown("external Agent generation cleanup failed".to_string())
}

fn lock_state<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ConnectionGuard {
    transport: Option<Arc<dyn AgentKernelRequestTransport>>,
}

impl ConnectionGuard {
    fn new(transport: Arc<dyn AgentKernelRequestTransport>) -> Self {
        Self {
            transport: Some(transport),
        }
    }

    fn disarm(&mut self) {
        self.transport = None;
    }

    fn fail(
        &mut self,
        error: ExternalAgentActivationError,
    ) -> (
        ExternalAgentActivationError,
        Option<Arc<dyn AgentKernelRequestTransport>>,
    ) {
        let Some(transport) = self.transport.take() else {
            return (error, None);
        };
        if transport.shutdown().is_err() {
            (ExternalAgentActivationError::Cleanup, Some(transport))
        } else {
            (error, None)
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if let Some(transport) = self.transport.take() {
            let _ = transport.shutdown();
        }
    }
}
