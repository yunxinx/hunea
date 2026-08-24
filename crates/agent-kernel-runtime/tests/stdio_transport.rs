use std::{
    num::NonZeroU64,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use agent_kernel_runtime::{
    ExternalAgentActivationError, ExternalAgentRuntime, ExternalAgentRuntimeOptions,
    StdioAgentKernelSource, StdioAgentKernelTransportOptions,
};
use runtime_domain::{
    agent::{
        AgentCommand, AgentEvent, AgentEventKind, AgentId, AgentRuntime, AgentRuntimeError,
        AgentTurnId, AgentTurnRequest,
    },
    event_notifier::RuntimeEventNotifier,
    session::{ConversationTurnRequest, RuntimeTarget},
};

const TRANSPORT_FAILURE_TEXT: &str = "External Agent kernel transport failed";

fn fixture_options(mode: &str) -> StdioAgentKernelTransportOptions {
    StdioAgentKernelTransportOptions::new(env!("CARGO_BIN_EXE_agent-kernel-stdio-fixture"))
        .arg(mode)
}

fn external_runtime(
    options: StdioAgentKernelTransportOptions,
    runtime_options: ExternalAgentRuntimeOptions,
) -> ExternalAgentRuntime {
    ExternalAgentRuntime::new(
        Arc::new(StdioAgentKernelSource::new(options)),
        runtime_options,
    )
}

fn default_runtime(mode: &str) -> ExternalAgentRuntime {
    external_runtime(
        fixture_options(mode),
        ExternalAgentRuntimeOptions::default(),
    )
}

fn short_runtime_options() -> ExternalAgentRuntimeOptions {
    ExternalAgentRuntimeOptions::new(
        NonZeroU64::new(500).expect("literal request deadline is non-zero"),
        NonZeroU64::new(100).expect("literal shutdown grace is non-zero"),
    )
}

fn submit(turn_id: u64) -> AgentCommand {
    AgentCommand::SubmitTurn {
        agent_id: AgentId::MAIN,
        turn_id: AgentTurnId::new(turn_id),
        request: Box::new(AgentTurnRequest::from_conversation_request(
            ConversationTurnRequest::new_user_text("local", "model", "private delivery"),
        )),
    }
}

fn collect_until(
    runtime: &mut ExternalAgentRuntime,
    condition: impl Fn(&[AgentEvent]) -> bool,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    for _ in 0..400 {
        events.extend(runtime.drain_events());
        if condition(&events) {
            return events;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("expected Agent events did not arrive");
}

#[test]
fn stdio_kernel_streams_permission_and_terminal_facts_without_polling_protocol() {
    let notifier = RuntimeEventNotifier::default();
    let wake_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = Arc::clone(&wake_count);
    let _binding = notifier.bind_callback(move || {
        captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let mut runtime = default_runtime("normal");
    runtime.activate(notifier).expect("stdio activation");
    runtime.dispatch(submit(41)).expect("submit receipt");

    let initial = collect_until(&mut runtime, |events| {
        events
            .iter()
            .any(|event| matches!(event.kind, AgentEventKind::PermissionRequested { .. }))
    });
    assert!(matches!(
        initial.as_slice(),
        [
            AgentEvent {
                kind: AgentEventKind::AssistantDelta { content },
                ..
            },
            AgentEvent {
                kind: AgentEventKind::PermissionRequested { .. },
                ..
            }
        ] if content == "remote stream"
    ));
    assert!(wake_count.load(std::sync::atomic::Ordering::SeqCst) > 0);

    runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(RuntimeTarget::provider("local", "model")),
            request_id: "permission-1".to_string(),
            option_id: Some("allow".to_string()),
        })
        .expect("permission receipt");
    let terminal = collect_until(&mut runtime, |events| {
        events.iter().any(|event| event.kind.is_terminal())
    });
    assert!(matches!(
        terminal.as_slice(),
        [AgentEvent {
            kind: AgentEventKind::TurnFinished { .. },
            ..
        }]
    ));
    assert!(!runtime.is_busy());
}

#[test]
fn stdio_event_before_receipt_is_correlated_and_delivered() {
    let mut runtime = default_runtime("event-before-receipt");
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    runtime.dispatch(submit(42)).expect("submit receipt");
    let events = collect_until(&mut runtime, |events| events.len() >= 2);
    assert!(matches!(
        &events[0].kind,
        AgentEventKind::AssistantDelta { content } if content == "remote stream"
    ));
    assert!(matches!(
        events[1].kind,
        AgentEventKind::PermissionRequested { .. }
    ));
}

#[test]
fn invalid_sequence_closes_only_the_adapter_with_one_safe_failure() {
    let mut runtime = default_runtime("invalid-sequence");
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    runtime.dispatch(submit(43)).expect("submit receipt");
    let events = collect_until(&mut runtime, |events| {
        events.iter().any(|event| event.kind.is_terminal())
    });
    let failures = events
        .iter()
        .filter_map(|event| match &event.kind {
            AgentEventKind::TurnFailed { message } => Some(message.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failures, [TRANSPORT_FAILURE_TEXT]);
    assert!(!format!("{events:?}").contains("invalid event body"));
}

#[test]
fn child_eof_and_stderr_fail_activation_without_leaking_child_output() {
    let mut eof = default_runtime("exit");
    assert!(eof.activate(RuntimeEventNotifier::default()).is_err());

    let mut stderr = default_runtime("stderr");
    let error = stderr
        .activate(RuntimeEventNotifier::default())
        .expect_err("stderr-only child must not initialize");
    assert_eq!(error, ExternalAgentActivationError::InitializationTransport);
    assert!(!error.to_string().contains("fixture stderr"));
    assert!(!format!("{stderr:?}").contains("fixture stderr"));
}

#[test]
fn shutdown_rejects_fresh_commands_and_is_idempotent() {
    let mut runtime = default_runtime("normal");
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    runtime.shutdown().expect("shutdown");
    runtime.shutdown().expect("repeated shutdown");
    assert!(matches!(
        runtime.dispatch(submit(44)),
        Err(AgentRuntimeError::Disposed)
    ));
}

#[test]
fn child_environment_is_cleared_before_explicit_values_are_applied() {
    let options =
        fixture_options("environment").environment("HUNEA_AGENT_KERNEL_ALLOWED", "explicit");
    let mut runtime = external_runtime(options, short_runtime_options());
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("fixture should observe only the explicit environment");
    runtime.shutdown().expect("shutdown");
}

#[test]
fn shutdown_terminates_and_reaps_an_uncooperative_child() {
    let pid_file = unique_pid_file();
    let options = fixture_options("ignore-shutdown").arg(pid_file.as_os_str().to_owned());
    let mut runtime = external_runtime(options, short_runtime_options());
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    let pid = std::fs::read_to_string(&pid_file)
        .expect("fixture pid should be written")
        .parse::<i32>()
        .expect("fixture pid should be numeric");

    runtime.shutdown().expect("bounded shutdown");
    assert!(!process_exists(pid));
    std::fs::remove_file(pid_file).expect("pid fixture cleanup");
}

#[test]
fn oversized_frame_and_bounded_event_queue_fail_closed() {
    let frame_options = fixture_options("oversized")
        .max_frame_bytes(1_024)
        .expect("frame limit");
    let mut oversized = external_runtime(frame_options, short_runtime_options());
    assert!(oversized.activate(RuntimeEventNotifier::default()).is_err());

    let event_options = fixture_options("event-flood")
        .queue_capacity(1_024)
        .expect("event queue capacity");
    let mut flooded = external_runtime(event_options, short_runtime_options());
    flooded
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    flooded.dispatch(submit(45)).expect("submit receipt");
    for _ in 0..400 {
        match flooded.dispatch(submit(46)) {
            Err(AgentRuntimeError::Busy) => std::thread::sleep(Duration::from_millis(5)),
            Err(AgentRuntimeError::CommandRejected(message))
                if message == TRANSPORT_FAILURE_TEXT =>
            {
                break;
            }
            Err(AgentRuntimeError::Disposed) => break,
            other => panic!("unexpected saturation state: {other:?}"),
        }
    }
    let events = flooded.drain_events();
    assert!(matches!(
        events.last(),
        Some(AgentEvent {
            kind: AgentEventKind::TurnFailed { message },
            ..
        }) if message == TRANSPORT_FAILURE_TEXT
    ));
}

#[test]
fn timed_out_response_is_tombstoned_without_closing_the_connection() {
    let options = ExternalAgentRuntimeOptions::new(
        NonZeroU64::new(50).expect("literal request deadline is non-zero"),
        NonZeroU64::new(100).expect("literal shutdown grace is non-zero"),
    );
    let mut runtime = external_runtime(fixture_options("late-response"), options);
    runtime
        .activate(RuntimeEventNotifier::default())
        .expect("stdio activation");
    assert!(matches!(
        runtime.dispatch(submit(46)),
        Err(AgentRuntimeError::CommandRejected(message)) if message.contains("timed out")
    ));

    std::thread::sleep(Duration::from_millis(200));
    runtime
        .dispatch(submit(47))
        .expect("late response must not close healthy transport");
    let events = collect_until(&mut runtime, |events| {
        events
            .iter()
            .any(|event| matches!(event.kind, AgentEventKind::PermissionRequested { .. }))
    });
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, AgentEventKind::AssistantDelta { .. }))
    );
}

fn unique_pid_file() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock should follow Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "hunea-agent-kernel-{}-{nonce}.pid",
        std::process::id()
    ))
}

fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 does not deliver a signal；这里只探测已知 child PID 是否仍存在。
    let status = unsafe { libc::kill(pid, 0) };
    status == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}
