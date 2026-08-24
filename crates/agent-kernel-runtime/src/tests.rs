use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use agent_kernel_protocol::{
    AgentKernelCapability, AgentKernelCommand, AgentKernelCommandParams, AgentKernelCommandReceipt,
    AgentKernelCommandResult, AgentKernelError, AgentKernelErrorCode, AgentKernelEvent,
    AgentKernelEventKind, AgentKernelEventNotification, AgentKernelInitializeParams,
    AgentKernelInitializeResult, AgentKernelMethod, AgentKernelPermissionOption,
    AgentKernelPermissionOptionKind, AgentKernelPermissionRequest, AgentKernelRequest,
    AgentKernelResponse, AgentKernelShutdownResult,
};
use runtime_domain::{
    agent::{
        AgentCommand, AgentCommandReceipt, AgentEvent, AgentEventKind, AgentId, AgentRuntime,
        AgentRuntimeError, AgentTurnId, AgentTurnRequest,
    },
    event_notifier::{RuntimeEventBinding, RuntimeEventNotifier},
    session::{ConversationTurnRequest, RuntimeTarget},
};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptMode {
    Normal,
    EventBeforeReceipt,
    EventThenReject,
    InvalidReceipt,
    RejectInitialize,
    RejectInitializeCleanupFails,
    ShutdownFails,
}

struct ScriptSource {
    mode: ScriptMode,
    connect_count: AtomicUsize,
    generations: Mutex<Vec<Arc<ScriptGeneration>>>,
    receipt_gate: Option<Arc<ReceiptGate>>,
}

struct ScriptGeneration {
    sender: Mutex<Option<AgentKernelEventSink>>,
    shutdowns: AtomicUsize,
    shutdown_attempts: AtomicUsize,
    initialize: Mutex<Option<AgentKernelInitializeParams>>,
    commands: Mutex<Vec<AgentKernelCommandParams>>,
}

struct ScriptTransport {
    mode: ScriptMode,
    generation: Arc<ScriptGeneration>,
    receipt_gate: Option<Arc<ReceiptGate>>,
}

#[derive(Default)]
struct ReceiptGate {
    is_released: Mutex<bool>,
    released: Condvar,
}

impl ScriptSource {
    fn new(mode: ScriptMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            connect_count: AtomicUsize::new(0),
            generations: Mutex::new(Vec::new()),
            receipt_gate: matches!(
                mode,
                ScriptMode::EventBeforeReceipt
                    | ScriptMode::EventThenReject
                    | ScriptMode::InvalidReceipt
            )
            .then(|| Arc::new(ReceiptGate::default())),
        })
    }

    fn generation(&self, index: usize) -> Arc<ScriptGeneration> {
        Arc::clone(
            self.generations
                .lock()
                .expect("generations")
                .get(index)
                .expect("generation should exist"),
        )
    }
}

impl AgentKernelSource for ScriptSource {
    fn connect(&self) -> Result<AgentKernelConnection, AgentKernelConnectError> {
        self.connect_count.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = AgentKernelEventStream::bounded(
            NonZeroUsize::new(16).expect("literal event capacity is non-zero"),
        );
        let generation = Arc::new(ScriptGeneration {
            sender: Mutex::new(Some(sender)),
            shutdowns: AtomicUsize::new(0),
            shutdown_attempts: AtomicUsize::new(0),
            initialize: Mutex::new(None),
            commands: Mutex::new(Vec::new()),
        });
        self.generations
            .lock()
            .expect("generations")
            .push(Arc::clone(&generation));
        Ok(AgentKernelConnection::new(
            ScriptTransport {
                mode: self.mode,
                generation,
                receipt_gate: self.receipt_gate.clone(),
            },
            receiver,
        ))
    }
}

impl AgentKernelRequestTransport for ScriptTransport {
    fn request(
        &self,
        request: AgentKernelRequest,
    ) -> Result<AgentKernelResponse, AgentKernelTransportError> {
        if self.generation.sender.lock().expect("sender").is_none() {
            return Err(AgentKernelTransportError::ShutDown);
        }
        let request_id = request.request_id().to_string();
        match request.method() {
            AgentKernelMethod::Initialize => {
                let params = request
                    .decode_params::<AgentKernelInitializeParams>()
                    .map_err(|_| AgentKernelTransportError::Protocol)?;
                *self.generation.initialize.lock().expect("initialize") = Some(params);
                if matches!(
                    self.mode,
                    ScriptMode::RejectInitialize | ScriptMode::RejectInitializeCleanupFails
                ) {
                    return Ok(AgentKernelResponse::failure(
                        request_id,
                        AgentKernelError::new(
                            AgentKernelErrorCode::Internal,
                            "remote initialization body sentinel",
                            false,
                        ),
                    ));
                }
                AgentKernelResponse::success(
                    request_id,
                    AgentKernelInitializeResult {
                        protocol: agent_kernel_protocol::PROTOCOL_NAME.to_string(),
                        version: agent_kernel_protocol::PROTOCOL_VERSION,
                        capabilities: vec![
                            AgentKernelCapability::Events,
                            AgentKernelCapability::Interrupt,
                            AgentKernelCapability::PermissionResponse,
                            AgentKernelCapability::StructuredErrors,
                        ],
                        accepted_host_capabilities: Vec::new(),
                    },
                )
                .map_err(|_| AgentKernelTransportError::Protocol)
            }
            AgentKernelMethod::AgentCommand => {
                let params = request
                    .decode_params::<AgentKernelCommandParams>()
                    .map_err(|_| AgentKernelTransportError::Protocol)?;
                self.generation
                    .commands
                    .lock()
                    .expect("commands")
                    .push(params.clone());
                if matches!(
                    self.mode,
                    ScriptMode::EventBeforeReceipt
                        | ScriptMode::EventThenReject
                        | ScriptMode::InvalidReceipt
                ) && let AgentKernelCommand::SubmitTurn {
                    agent_id,
                    turn_id,
                    request,
                } = &params.command
                {
                    self.generation.send(AgentKernelEventNotification::new(
                        1,
                        params.command_id,
                        AgentKernelEvent {
                            agent_id: *agent_id,
                            turn_id: *turn_id,
                            target: request.target.clone(),
                            kind: AgentKernelEventKind::AssistantDelta {
                                content: "staged content".to_string(),
                            },
                        },
                    ));
                    self.receipt_gate
                        .as_ref()
                        .expect("staged-event mode has a receipt gate")
                        .wait();
                }
                if self.mode == ScriptMode::EventThenReject {
                    return Ok(AgentKernelResponse::failure(
                        request_id,
                        AgentKernelError::new(
                            AgentKernelErrorCode::CommandRejected,
                            "remote rejection body",
                            false,
                        ),
                    ));
                }
                let receipt = match &params.command {
                    AgentKernelCommand::SubmitTurn {
                        turn_id, request, ..
                    } => AgentKernelCommandReceipt::TurnStarted {
                        turn_id: if self.mode == ScriptMode::InvalidReceipt {
                            turn_id.saturating_add(1)
                        } else {
                            *turn_id
                        },
                        target: request.target.clone(),
                        activity_label: request.target.model_id.clone(),
                    },
                    AgentKernelCommand::Interrupt { target, .. } => {
                        AgentKernelCommandReceipt::Interrupted {
                            target: target.clone(),
                        }
                    }
                    AgentKernelCommand::RespondPermission { .. } => {
                        AgentKernelCommandReceipt::Accepted
                    }
                };
                AgentKernelResponse::success(
                    request_id,
                    AgentKernelCommandResult {
                        command_id: params.command_id,
                        receipt,
                    },
                )
                .map_err(|_| AgentKernelTransportError::Protocol)
            }
            AgentKernelMethod::Shutdown => AgentKernelResponse::success(
                request_id,
                AgentKernelShutdownResult { drained: true },
            )
            .map_err(|_| AgentKernelTransportError::Protocol),
        }
    }

    fn shutdown(&self) -> Result<(), AgentKernelTransportError> {
        let attempt = self
            .generation
            .shutdown_attempts
            .fetch_add(1, Ordering::SeqCst);
        let was_open = self
            .generation
            .sender
            .lock()
            .expect("sender")
            .take()
            .is_some();
        if was_open {
            self.generation.shutdowns.fetch_add(1, Ordering::SeqCst);
        }
        if was_open
            && (self.mode == ScriptMode::ShutdownFails
                || (self.mode == ScriptMode::RejectInitializeCleanupFails && attempt == 0))
        {
            Err(AgentKernelTransportError::Unavailable)
        } else {
            Ok(())
        }
    }
}

impl ReceiptGate {
    fn wait(&self) {
        let mut is_released = self.is_released.lock().expect("receipt gate");
        while !*is_released {
            is_released = self.released.wait(is_released).expect("receipt gate wait");
        }
    }

    fn release(&self) {
        *self.is_released.lock().expect("receipt gate") = true;
        self.released.notify_all();
    }
}

impl ScriptGeneration {
    fn send(&self, event: AgentKernelEventNotification) {
        self.sender
            .lock()
            .expect("sender")
            .as_ref()
            .expect("generation is open")
            .send(event)
            .expect("event should send");
    }

    fn disconnect(&self) {
        self.sender.lock().expect("sender").take();
    }

    fn command(&self, index: usize) -> AgentKernelCommandParams {
        self.commands.lock().expect("commands")[index].clone()
    }
}

fn runtime(source: Arc<ScriptSource>) -> ExternalAgentRuntime {
    ExternalAgentRuntime::new(source, ExternalAgentRuntimeOptions::default())
}

fn submit() -> AgentCommand {
    AgentCommand::SubmitTurn {
        agent_id: AgentId::MAIN,
        turn_id: AgentTurnId::new(7),
        request: Box::new(AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "model", "hello"),
        )),
    }
}

fn event_for_command(
    sequence: u64,
    command: &AgentKernelCommandParams,
    kind: AgentKernelEventKind,
) -> AgentKernelEventNotification {
    let AgentKernelCommand::SubmitTurn {
        agent_id,
        turn_id,
        request,
    } = &command.command
    else {
        panic!("expected submit command")
    };
    AgentKernelEventNotification::new(
        sequence,
        command.command_id,
        AgentKernelEvent {
            agent_id: *agent_id,
            turn_id: *turn_id,
            target: request.target.clone(),
            kind,
        },
    )
}

fn notifier_counter() -> (RuntimeEventNotifier, Arc<AtomicUsize>, RuntimeEventBinding) {
    let notifier = RuntimeEventNotifier::default();
    let wakes = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&wakes);
    let binding = notifier.bind_callback(move || {
        captured.fetch_add(1, Ordering::SeqCst);
    });
    (notifier, wakes, binding)
}

fn release_receipt_after_event_is_staged(
    source: &ScriptSource,
    runtime: &ExternalAgentRuntime,
) -> (
    std::thread::JoinHandle<()>,
    Arc<std::sync::atomic::AtomicBool>,
) {
    let shared = Arc::clone(
        &runtime
            .generation
            .as_ref()
            .expect("runtime generation")
            .shared,
    );
    let receipt_gate = Arc::clone(
        source
            .receipt_gate
            .as_ref()
            .expect("staged-event mode has a receipt gate"),
    );
    let was_staged = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&was_staged);
    let handle = std::thread::spawn(move || {
        for _ in 0..200 {
            let has_staged_event = lock_state(&shared.state).commands.values().any(|command| {
                matches!(&command.phase, CommandPhase::Pending { events } if !events.is_empty())
            });
            if has_staged_event {
                observed.store(true, Ordering::SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        receipt_gate.release();
    });
    (handle, was_staged)
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("condition did not become true");
}

fn wait_until_failed(runtime: &ExternalAgentRuntime) {
    let shared = Arc::clone(
        &runtime
            .generation
            .as_ref()
            .expect("active generation")
            .shared,
    );
    wait_until(|| lock_state(&shared.state).is_failed);
}

fn take_one_safe_failure(runtime: &mut ExternalAgentRuntime) -> Vec<AgentEvent> {
    wait_until_failed(runtime);
    let events = runtime.drain_events();
    let terminal = events
        .iter()
        .filter(|event| event.kind.is_terminal())
        .collect::<Vec<_>>();
    assert!(matches!(
        terminal.as_slice(),
        [runtime_domain::agent::AgentEvent {
            kind: AgentEventKind::TurnFailed { message },
            ..
        }] if message == TRANSPORT_FAILURE_TEXT
    ));
    events
}

#[test]
fn event_stream_construction_is_bounded_and_reports_saturation() {
    let (sink, stream) = AgentKernelEventStream::bounded(
        NonZeroUsize::new(1).expect("literal event capacity is non-zero"),
    );
    let event = AgentKernelEventNotification::new(
        1,
        1,
        AgentKernelEvent {
            agent_id: 1,
            turn_id: 1,
            target: agent_kernel_protocol::AgentKernelTarget {
                provider_id: "local".to_string(),
                model_id: "model".to_string(),
            },
            kind: AgentKernelEventKind::TurnInterrupted,
        },
    );

    sink.try_send(event.clone()).expect("first event fits");
    assert_eq!(
        sink.try_send(event),
        Err(AgentKernelEventSendError::Saturated)
    );
    drop(stream);
}

#[test]
fn activation_errors_are_closed_and_redacted() {
    let source = ScriptSource::new(ScriptMode::RejectInitialize);
    let mut runtime = runtime(Arc::clone(&source));
    let error = runtime
        .activate(RuntimeEventNotifier::default())
        .expect_err("remote rejection must fail activation");

    assert_eq!(error, ExternalAgentActivationError::InitializationRejected);
    let diagnostic = format!("{error:?}\n{error}");
    assert!(!diagnostic.contains("remote initialization body sentinel"));
    assert_eq!(source.generation(0).shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_activation_retains_cleanup_owner_until_shutdown_retry_succeeds() {
    let source = ScriptSource::new(ScriptMode::RejectInitializeCleanupFails);
    let mut runtime = runtime(Arc::clone(&source));
    assert_eq!(
        runtime.activate(RuntimeEventNotifier::default()),
        Err(ExternalAgentActivationError::Cleanup)
    );
    assert_eq!(
        runtime.activate(RuntimeEventNotifier::default()),
        Err(ExternalAgentActivationError::CleanupPending)
    );

    runtime
        .shutdown()
        .expect("final shutdown must retry failed activation cleanup");
    assert_eq!(
        source
            .generation(0)
            .shutdown_attempts
            .load(Ordering::SeqCst),
        2
    );
}

#[test]
fn construction_has_no_effect_and_activation_negotiates_no_host_authority() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let mut runtime = runtime(Arc::clone(&source));
    assert_eq!(source.connect_count.load(Ordering::SeqCst), 0);

    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation should succeed");
    assert_eq!(source.connect_count.load(Ordering::SeqCst), 1);
    let initialize = source
        .generation(0)
        .initialize
        .lock()
        .expect("initialize")
        .clone()
        .expect("initialize should be captured");
    assert!(initialize.host_capabilities.is_empty());
    assert_eq!(initialize.capabilities.len(), 4);
}

#[test]
fn validated_event_is_queued_before_wake_and_drains_fifo() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let (notifier, wakes, _binding) = notifier_counter();
    let mut runtime = runtime(Arc::clone(&source));
    runtime.activate(notifier).expect("activation");
    assert!(matches!(
        runtime.dispatch(submit()).expect("submit"),
        AgentCommandReceipt::TurnStarted { .. }
    ));
    assert_eq!(wakes.load(Ordering::SeqCst), 0);

    let generation = source.generation(0);
    let command = generation.command(0);
    generation.send(event_for_command(
        1,
        &command,
        AgentKernelEventKind::AssistantDelta {
            content: "first".to_string(),
        },
    ));
    generation.send(event_for_command(
        2,
        &command,
        AgentKernelEventKind::ReasoningDelta {
            content: "second".to_string(),
        },
    ));
    wait_until(|| wakes.load(Ordering::SeqCst) > 0);
    let events = runtime.drain_events();
    assert!(matches!(
        &events[0].kind,
        AgentEventKind::AssistantDelta { content } if content == "first"
    ));
    assert!(matches!(
        &events[1].kind,
        AgentEventKind::ReasoningDelta { content } if content == "second"
    ));
}

#[test]
fn event_before_receipt_is_staged_and_wakes_only_after_receipt_acceptance() {
    let source = ScriptSource::new(ScriptMode::EventBeforeReceipt);
    let (notifier, wakes, _binding) = notifier_counter();
    let mut runtime = runtime(Arc::clone(&source));
    runtime.activate(notifier).expect("activation");
    let (release, was_staged) = release_receipt_after_event_is_staged(&source, &runtime);
    runtime.dispatch(submit()).expect("submit");
    release.join().expect("receipt release worker");

    assert!(was_staged.load(Ordering::SeqCst));
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        runtime.drain_events().as_slice(),
        [runtime_domain::agent::AgentEvent {
            kind: AgentEventKind::AssistantDelta { content },
            ..
        }] if content == "staged content"
    ));
}

#[test]
fn rejected_or_invalid_receipt_discards_staged_events_without_wake() {
    for mode in [ScriptMode::EventThenReject, ScriptMode::InvalidReceipt] {
        let source = ScriptSource::new(mode);
        let (notifier, wakes, _binding) = notifier_counter();
        let mut runtime = runtime(Arc::clone(&source));
        runtime.activate(notifier).expect("activation");
        let (release, was_staged) = release_receipt_after_event_is_staged(&source, &runtime);
        assert!(runtime.dispatch(submit()).is_err());
        release.join().expect("receipt release worker");
        assert!(was_staged.load(Ordering::SeqCst));
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        assert!(runtime.drain_events().is_empty());
    }
}

#[test]
fn permission_and_terminal_events_correlate_to_their_commands() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let (notifier, wakes, _binding) = notifier_counter();
    let mut runtime = runtime(Arc::clone(&source));
    runtime.activate(notifier).expect("activation");
    runtime.dispatch(submit()).expect("submit");
    let generation = source.generation(0);
    let submit = generation.command(0);
    generation.send(event_for_command(
        1,
        &submit,
        AgentKernelEventKind::PermissionRequested {
            request: AgentKernelPermissionRequest {
                request_id: "permission-1".to_string(),
                title: None,
                tool_activity: None,
                options: vec![AgentKernelPermissionOption {
                    option_id: "allow".to_string(),
                    name: "Allow".to_string(),
                    kind: AgentKernelPermissionOptionKind::AllowOnce,
                }],
            },
        },
    ));
    wait_until(|| wakes.load(Ordering::SeqCst) > 0);
    assert!(matches!(
        runtime.drain_events()[0].kind,
        AgentEventKind::PermissionRequested { .. }
    ));

    runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(RuntimeTarget::provider("local", "model")),
            request_id: "permission-1".to_string(),
            option_id: Some("allow".to_string()),
        })
        .expect("permission response");
    let permission = generation.command(1);
    let AgentKernelCommand::RespondPermission { .. } = permission.command else {
        panic!("expected permission command")
    };
    generation.send(AgentKernelEventNotification::new(
        2,
        permission.command_id,
        AgentKernelEvent {
            agent_id: 1,
            turn_id: 7,
            target: agent_kernel_protocol::AgentKernelTarget {
                provider_id: "local".to_string(),
                model_id: "model".to_string(),
            },
            kind: AgentKernelEventKind::TurnInterrupted,
        },
    ));
    wait_until(|| wakes.load(Ordering::SeqCst) > 1);
    assert!(matches!(
        runtime.drain_events()[0].kind,
        AgentEventKind::TurnInterrupted
    ));
    assert!(!runtime.is_busy());
}

#[test]
fn interrupt_is_dispatched_and_correlated_to_the_active_turn() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let mut runtime = runtime(Arc::clone(&source));
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");
    runtime.dispatch(submit()).expect("submit");

    assert!(matches!(
        runtime
            .dispatch(AgentCommand::Interrupt {
                agent_id: AgentId::MAIN,
                target: Some(RuntimeTarget::provider("local", "model")),
            })
            .expect("interrupt receipt"),
        AgentCommandReceipt::Interrupted { .. }
    ));
    let generation = source.generation(0);
    let interrupt = generation.command(1);
    assert!(matches!(
        interrupt.command,
        AgentKernelCommand::Interrupt { .. }
    ));
    generation.send(AgentKernelEventNotification::new(
        1,
        interrupt.command_id,
        AgentKernelEvent {
            agent_id: 1,
            turn_id: 7,
            target: agent_kernel_protocol::AgentKernelTarget {
                provider_id: "local".to_string(),
                model_id: "model".to_string(),
            },
            kind: AgentKernelEventKind::TurnInterrupted,
        },
    ));
    wait_until(|| runtime.is_busy());
    wait_until(|| {
        !lock_state(
            &runtime
                .generation
                .as_ref()
                .expect("active generation")
                .shared
                .state,
        )
        .ready
        .is_empty()
    });
    assert!(matches!(
        runtime.drain_events().as_slice(),
        [runtime_domain::agent::AgentEvent {
            kind: AgentEventKind::TurnInterrupted,
            ..
        }]
    ));
}

#[test]
fn unknown_command_and_identity_mismatch_fail_closed() {
    for mutate in [
        |event: &mut AgentKernelEventNotification| event.command_id += 1,
        |event: &mut AgentKernelEventNotification| event.event.turn_id += 1,
    ] {
        let source = ScriptSource::new(ScriptMode::Normal);
        let mut runtime = runtime(Arc::clone(&source));
        runtime
            .activate(RuntimeEventNotifier::default())
            .expect("activation");
        runtime.dispatch(submit()).expect("submit");
        let generation = source.generation(0);
        let command = generation.command(0);
        let mut event = event_for_command(
            1,
            &command,
            AgentKernelEventKind::AssistantDelta {
                content: "must not be delivered".to_string(),
            },
        );
        mutate(&mut event);
        generation.send(event);

        let _ = take_one_safe_failure(&mut runtime);
    }
}

#[test]
fn duplicate_permission_and_event_after_terminal_replace_success_with_safe_failure() {
    {
        let source = ScriptSource::new(ScriptMode::Normal);
        let mut runtime = runtime(Arc::clone(&source));
        runtime
            .activate(RuntimeEventNotifier::default())
            .expect("activation");
        runtime.dispatch(submit()).expect("submit");
        let generation = source.generation(0);
        let command = generation.command(0);
        let permission = AgentKernelEventKind::PermissionRequested {
            request: AgentKernelPermissionRequest {
                request_id: "permission-duplicate".to_string(),
                title: None,
                tool_activity: None,
                options: vec![AgentKernelPermissionOption {
                    option_id: "allow".to_string(),
                    name: "Allow".to_string(),
                    kind: AgentKernelPermissionOptionKind::AllowOnce,
                }],
            },
        };
        generation.send(event_for_command(1, &command, permission.clone()));
        generation.send(event_for_command(2, &command, permission));
        let _ = take_one_safe_failure(&mut runtime);
    }

    let source = ScriptSource::new(ScriptMode::Normal);
    let mut second_runtime = runtime(Arc::clone(&source));
    second_runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");
    second_runtime.dispatch(submit()).expect("submit");
    let generation = source.generation(0);
    let command = generation.command(0);
    generation.send(event_for_command(
        1,
        &command,
        AgentKernelEventKind::TurnInterrupted,
    ));
    generation.send(event_for_command(
        2,
        &command,
        AgentKernelEventKind::AssistantDelta {
            content: "must not follow terminal".to_string(),
        },
    ));
    let events = take_one_safe_failure(&mut second_runtime);
    assert!(!format!("{events:?}").contains("must not follow terminal"));
}

#[test]
fn invalid_sequence_fails_closed_with_one_safe_terminal_fact() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let (notifier, wakes, _binding) = notifier_counter();
    let mut runtime = runtime(Arc::clone(&source));
    runtime.activate(notifier).expect("activation");
    runtime.dispatch(submit()).expect("submit");
    let generation = source.generation(0);
    let command = generation.command(0);
    generation.send(event_for_command(
        2,
        &command,
        AgentKernelEventKind::AssistantDelta {
            content: "must not arrive".to_string(),
        },
    ));
    wait_until(|| wakes.load(Ordering::SeqCst) > 0);
    let events = runtime.drain_events();
    assert!(matches!(
        events.as_slice(),
        [runtime_domain::agent::AgentEvent {
            kind: AgentEventKind::TurnFailed { message },
            ..
        }] if message == TRANSPORT_FAILURE_TEXT
    ));
    assert!(!format!("{events:?}").contains("must not arrive"));
    assert_eq!(generation.shutdowns.load(Ordering::SeqCst), 1);
}

#[test]
fn transport_disconnect_fails_only_the_active_adapter() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let (notifier, wakes, _binding) = notifier_counter();
    let mut runtime = runtime(Arc::clone(&source));
    runtime.activate(notifier).expect("activation");
    runtime.dispatch(submit()).expect("submit");
    source.generation(0).disconnect();

    wait_until(|| wakes.load(Ordering::SeqCst) > 0);
    assert!(matches!(
        runtime.drain_events()[0].kind,
        AgentEventKind::TurnFailed { .. }
    ));
}

#[test]
fn suspend_discards_old_generation_and_reactivation_connects_fresh() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let mut runtime = runtime(Arc::clone(&source));
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");
    runtime.dispatch(submit()).expect("submit");
    let old = source.generation(0);
    let old_command = old.command(0);
    let stale_sink = old
        .sender
        .lock()
        .expect("sender")
        .as_ref()
        .expect("old generation is open")
        .clone();
    runtime.suspend().expect("suspend");
    assert!(old.sender.lock().expect("sender").is_none());
    assert!(runtime.drain_events().is_empty());
    assert_eq!(old.shutdowns.load(Ordering::SeqCst), 1);

    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("reactivation");
    assert_eq!(source.connect_count.load(Ordering::SeqCst), 2);
    assert!(old.sender.lock().expect("sender").as_ref().is_none());
    assert_eq!(
        stale_sink.try_send(event_for_command(
            1,
            &old_command,
            AgentKernelEventKind::TurnInterrupted,
        )),
        Err(AgentKernelEventSendError::Closed)
    );
    assert!(runtime.drain_events().is_empty());
}

#[test]
fn final_shutdown_is_idempotent_and_disposes_adapter() {
    let source = ScriptSource::new(ScriptMode::Normal);
    let mut runtime = runtime(Arc::clone(&source));
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");
    runtime.shutdown().expect("shutdown");
    runtime.shutdown().expect("repeated shutdown");
    assert_eq!(source.generation(0).shutdowns.load(Ordering::SeqCst), 1);
    assert!(matches!(
        runtime.dispatch(submit()),
        Err(AgentRuntimeError::Disposed)
    ));
}

#[test]
fn final_shutdown_retries_cleanup_until_the_finalizer_succeeds() {
    let source = ScriptSource::new(ScriptMode::ShutdownFails);
    let mut runtime = runtime(Arc::clone(&source));
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");

    let first = runtime.shutdown().expect_err("first shutdown must fail");
    assert!(first.to_string().contains("cleanup failed"));
    runtime
        .shutdown()
        .expect("repeated shutdown must retry the finalizer");
    runtime
        .shutdown()
        .expect("completed shutdown is idempotent");
    assert_eq!(source.generation(0).shutdowns.load(Ordering::SeqCst), 1);
    assert_eq!(
        source
            .generation(0)
            .shutdown_attempts
            .load(Ordering::SeqCst),
        2
    );
}

#[test]
fn failed_suspend_closes_admission_until_cleanup_retry_completes() {
    let source = ScriptSource::new(ScriptMode::ShutdownFails);
    let mut runtime = runtime(Arc::clone(&source));
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("activation");

    runtime.suspend().expect_err("first suspend must fail");
    assert!(matches!(
        runtime.dispatch(submit()),
        Err(AgentRuntimeError::Disposed)
    ));
    runtime
        .suspend()
        .expect("second suspend must retry cleanup");
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("cleanup completion permits fresh activation");
    assert_eq!(source.connect_count.load(Ordering::SeqCst), 2);
}
