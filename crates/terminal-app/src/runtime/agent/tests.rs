use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use conversation_runtime::RuntimeEventNotifier;
use provider_protocol::{ConversationItem, Role};
use runtime_domain::{
    prompt_assembly::PromptSourceOrigin,
    provider::ProviderKind,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimePermissionOption,
        RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity,
        RuntimeToolActivityStatus, RuntimeToolKind, TranscriptSkillBinding, TranscriptUserMessage,
    },
};
use tool_runtime::ToolExecutorRegistry;

use super::{
    AgentCommand, AgentEvent, AgentEventKind, AgentId, AgentRuntime, AgentRuntimeError,
    AgentTurnId, AgentTurnRequest, NativeAgentRuntime,
};
use crate::runtime::{AppRuntimeOptions, prompt_assembly::PromptAssemblySessionSnapshot};

fn native_runtime(event_notifier: RuntimeEventNotifier) -> NativeAgentRuntime {
    NativeAgentRuntime::new(
        &AppRuntimeOptions {
            runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
                0,
                Vec::new(),
                1,
            ),
            ..AppRuntimeOptions::default()
        },
        ToolExecutorRegistry::default(),
        Vec::new(),
        PromptAssemblySessionSnapshot::default(),
        event_notifier,
    )
    .expect("native Agent runtime should initialize")
}

fn failing_turn_request() -> AgentTurnRequest {
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "hello"),
    ))
}

fn replay_request() -> AgentTurnRequest {
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "replay",
        ProviderKind::OpenAiCompatible,
        "fixture-model",
        None,
        None,
        None,
        ConversationItem::text(Role::User, "replay this turn"),
    ))
}

fn replay_fixture() -> super::replay::ReplayFixture {
    super::replay::ReplayFixture::new(vec![
        AgentEventKind::AssistantDelta {
            content: "replayed".to_string(),
        },
        AgentEventKind::TurnFinished {
            response: ConversationResponse::assistant_text("replayed"),
            metrics: None,
            context_usage: None,
        },
    ])
    .expect("test replay fixture should be valid")
}

fn permission_fixture() -> super::replay::ReplayFixture {
    super::replay::ReplayFixture::new(vec![
        AgentEventKind::AssistantDelta {
            content: "before approval".to_string(),
        },
        AgentEventKind::PermissionRequested {
            request: RuntimePermissionRequest::new(
                "replay-permission",
                Some("Approve fixture action".to_string()),
                vec![RuntimePermissionOption::new(
                    "allow-once",
                    "Allow once",
                    RuntimePermissionOptionKind::AllowOnce,
                )],
            ),
        },
        AgentEventKind::ToolActivityStarted {
            activity: RuntimeToolActivity {
                activity_id: "fixture-tool".to_string(),
                title: "Fixture tool".to_string(),
                kind: RuntimeToolKind::Read,
                status: RuntimeToolActivityStatus::Completed,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
        AgentEventKind::TurnFinished {
            response: ConversationResponse::assistant_text("approved"),
            metrics: None,
            context_usage: None,
        },
    ])
    .expect("permission replay fixture should be valid")
}

fn collect_until_terminal(runtime: &mut dyn AgentRuntime) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        events.extend(runtime.drain_events());
        if events.iter().any(|event| event.kind.is_terminal()) {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    events
}

fn assert_shared_identity_and_terminal(
    runtime: &mut dyn AgentRuntime,
    request: AgentTurnRequest,
    expected_agent: AgentId,
    expected_turn: AgentTurnId,
) {
    let target = request.target();
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: expected_agent,
            turn_id: expected_turn,
            request: Box::new(request),
        })
        .expect("adapter should admit the shared contract turn");
    let events = collect_until_terminal(runtime);
    assert!(!events.is_empty(), "adapter should publish fixture facts");
    assert!(events.iter().any(|event| event.kind.is_terminal()));
    assert!(events.iter().all(|event| {
        event.agent_id == expected_agent && event.turn_id == expected_turn && event.target == target
    }));
    assert!(
        runtime.drain_events().is_empty(),
        "terminal fact must close the turn against late facts"
    );
}

fn assert_shared_lifecycle_contract(runtime: &mut dyn AgentRuntime) {
    assert!(runtime.drain_events().is_empty());
    let error = runtime
        .dispatch(AgentCommand::Interrupt {
            agent_id: AgentId::new(999),
            target: None,
        })
        .expect_err("unknown Agent handles must be rejected");
    assert!(matches!(error, AgentRuntimeError::UnknownAgent));
    runtime.shutdown().expect("adapter should dispose cleanly");
    runtime
        .shutdown()
        .expect("adapter shutdown should be idempotent");
    let error = runtime
        .dispatch(AgentCommand::Interrupt {
            agent_id: AgentId::MAIN,
            target: None,
        })
        .expect_err("disposed adapter must reject commands");
    assert!(matches!(error, AgentRuntimeError::Disposed));
    assert!(runtime.drain_events().is_empty());
}

fn assert_shared_busy_and_interrupt_contract(
    runtime: &mut dyn AgentRuntime,
    target: RuntimeTarget,
    next_request: AgentTurnRequest,
) {
    let busy = runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(71),
            request: Box::new(next_request.clone()),
        })
        .expect_err("active adapter must reject a second submit");
    assert!(matches!(busy, AgentRuntimeError::Busy));
    let receipt = runtime
        .dispatch(AgentCommand::Interrupt {
            agent_id: AgentId::MAIN,
            target: Some(target),
        })
        .expect("interrupt should cancel the active contract turn");
    assert!(matches!(
        receipt,
        super::AgentCommandReceipt::Interrupted { target: Some(_) }
    ));
    let interrupted = runtime.drain_events();
    assert!(
        interrupted
            .iter()
            .all(|event| matches!(event.kind, AgentEventKind::TurnInterrupted))
    );
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(72),
            request: Box::new(next_request),
        })
        .expect("adapter should accept work after interrupt quiescence");
    runtime
        .shutdown()
        .expect("adapter should dispose after interrupt contract");
}

#[test]
fn request_debug_redacts_delivery_controls_and_native_credentials() {
    let request = AgentTurnRequest::from_conversation_request(
        ConversationTurnRequest::new_user_source_message(
            "provider",
            ProviderKind::OpenAi,
            "model",
            Some("https://credential.example/v1".to_string()),
            Some(runtime_domain::provider::ProviderApiKey::new("secret-key")),
            Some("SECRET_ENV".to_string()),
            TranscriptUserMessage {
                content: "visible-sentinel".to_string(),
                attachments: Vec::new(),
                skill_bindings: vec![TranscriptSkillBinding {
                    skill_name: "private-skill".to_string(),
                    origin: PromptSourceOrigin::Project,
                    skill_path: "/private/SKILL.md".to_string(),
                    start_char: 0,
                    end_char: 1,
                }],
                custom_prompt_bindings: Vec::new(),
            },
        ),
    );

    let debug = format!("{request:?}");
    for secret in [
        "visible-sentinel",
        "private-skill",
        "/private/SKILL.md",
        "credential.example",
        "secret-key",
        "SECRET_ENV",
    ] {
        assert!(!debug.contains(secret), "debug output leaked {secret}");
    }
    assert!(debug.contains("content_chars"));
    assert!(debug.contains("skill_binding_count"));
}

#[test]
fn native_runtime_wakes_only_after_an_identified_event_is_available() {
    let (wake_tx, wake_rx) = mpsc::channel();
    let notifier = RuntimeEventNotifier::default();
    let _binding = notifier.bind_callback(move || {
        let _ = wake_tx.send(());
    });
    let mut runtime = native_runtime(notifier);
    let turn_id = AgentTurnId::new(17);
    let target = failing_turn_request().target();
    let runtime_contract: &mut dyn AgentRuntime = &mut runtime;

    runtime_contract
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id,
            request: Box::new(failing_turn_request()),
        })
        .expect("native turn should be admitted before provider preflight");
    wake_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("accepted turn should wake after publishing an event");

    let events = collect_until_terminal(runtime_contract);
    assert!(events.iter().any(|event| event.kind.is_terminal()));
    assert!(events.iter().all(|event| {
        event.agent_id == AgentId::MAIN && event.turn_id == turn_id && event.target == target
    }));
    runtime_contract
        .shutdown()
        .expect("native runtime should shut down cleanly");
}

#[test]
fn shared_identity_and_terminal_contract_runs_for_native_and_replay() {
    let mut native = native_runtime(RuntimeEventNotifier::default());
    assert_shared_identity_and_terminal(
        &mut native,
        failing_turn_request(),
        AgentId::MAIN,
        AgentTurnId::new(31),
    );
    native
        .shutdown()
        .expect("native should shut down after contract");

    let mut replay =
        super::replay::ReplayAgentRuntime::new(replay_fixture(), RuntimeEventNotifier::default());
    assert_shared_identity_and_terminal(
        &mut replay,
        replay_request(),
        AgentId::MAIN,
        AgentTurnId::new(32),
    );
    replay
        .shutdown()
        .expect("replay should shut down after contract");
}

#[test]
fn shared_lifecycle_contract_runs_for_native_and_replay() {
    let mut native = native_runtime(RuntimeEventNotifier::default());
    assert_shared_lifecycle_contract(&mut native);

    let mut replay =
        super::replay::ReplayAgentRuntime::new(replay_fixture(), RuntimeEventNotifier::default());
    assert_shared_lifecycle_contract(&mut replay);
}

#[test]
fn shared_busy_and_interrupt_contract_runs_for_native_and_replay() {
    let native_request = failing_turn_request();
    let native_target = native_request.target();
    let mut native = native_runtime(RuntimeEventNotifier::default());
    native.set_pending_turn_for_test(native_request.native_request.clone());
    assert_shared_busy_and_interrupt_contract(&mut native, native_target, failing_turn_request());

    let active_replay_request = replay_request();
    let replay_target = active_replay_request.target();
    let mut replay = super::replay::ReplayAgentRuntime::new(
        permission_fixture(),
        RuntimeEventNotifier::default(),
    );
    replay
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(70),
            request: Box::new(active_replay_request),
        })
        .expect("replay should enter the active contract state");
    assert_shared_busy_and_interrupt_contract(&mut replay, replay_target, replay_request());
}

#[test]
fn replay_fixture_validation_rejects_unsafe_sequences() {
    let terminal = AgentEventKind::TurnFailed {
        message: "failure".to_string(),
    };
    assert!(matches!(
        super::replay::ReplayFixture::new(Vec::new()),
        Err(super::replay::ReplayFixtureError::MissingOrMultipleTerminalFacts)
    ));
    assert!(matches!(
        super::replay::ReplayFixture::new(vec![terminal.clone(), terminal.clone()]),
        Err(super::replay::ReplayFixtureError::MissingOrMultipleTerminalFacts)
    ));
    assert!(matches!(
        super::replay::ReplayFixture::new(vec![
            terminal.clone(),
            AgentEventKind::Thinking { is_thinking: false }
        ]),
        Err(super::replay::ReplayFixtureError::TerminalFactIsNotLast)
    ));
    let permission = RuntimePermissionRequest::new("duplicate", None, Vec::new());
    assert!(matches!(
        super::replay::ReplayFixture::new(vec![
            AgentEventKind::PermissionRequested {
                request: permission.clone(),
            },
            AgentEventKind::PermissionRequested { request: permission },
            terminal,
        ]),
        Err(super::replay::ReplayFixtureError::DuplicatePermissionRequest(id)) if id == "duplicate"
    ));
}

#[test]
fn replay_permission_gate_preserves_order_and_validates_target_request_and_option() {
    let mut runtime = super::replay::ReplayAgentRuntime::new(
        permission_fixture(),
        RuntimeEventNotifier::default(),
    );
    let target = replay_request().target();
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(41),
            request: Box::new(replay_request()),
        })
        .expect("permission replay should start");
    let first = runtime.drain_events();
    assert!(matches!(first.first(), Some(AgentEvent {
        kind: AgentEventKind::AssistantDelta { content }, ..
    }) if content == "before approval"));
    assert!(matches!(first.last(), Some(AgentEvent {
        kind: AgentEventKind::PermissionRequested { request }, ..
    }) if request.request_id == "replay-permission"));
    assert!(runtime.drain_events().is_empty());

    let wrong_target = runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(RuntimeTarget::provider("other", "model")),
            request_id: "replay-permission".to_string(),
            option_id: Some("allow-once".to_string()),
        })
        .expect_err("wrong permission target must be rejected");
    assert!(matches!(
        wrong_target,
        AgentRuntimeError::CommandRejected(_)
    ));
    let wrong_request = runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(target.clone()),
            request_id: "other-permission".to_string(),
            option_id: Some("allow-once".to_string()),
        })
        .expect_err("wrong permission request id must be rejected");
    assert!(matches!(
        wrong_request,
        AgentRuntimeError::CommandRejected(_)
    ));
    let wrong_option = runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(target.clone()),
            request_id: "replay-permission".to_string(),
            option_id: Some("unknown".to_string()),
        })
        .expect_err("unknown permission option must be rejected");
    assert!(matches!(
        wrong_option,
        AgentRuntimeError::CommandRejected(_)
    ));

    runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(target),
            request_id: "replay-permission".to_string(),
            option_id: None,
        })
        .expect("matching permission cancellation should resume replay");
    let remaining = runtime.drain_events();
    assert!(matches!(
        remaining.first(),
        Some(AgentEvent {
            kind: AgentEventKind::ToolActivityStarted { .. },
            ..
        })
    ));
    assert!(remaining.iter().any(|event| event.kind.is_terminal()));
    assert!(runtime.drain_events().is_empty());
}

#[test]
fn replay_interrupt_replaces_all_undelivered_facts_with_one_terminal_fact() {
    let mut runtime = super::replay::ReplayAgentRuntime::new(
        permission_fixture(),
        RuntimeEventNotifier::default(),
    );
    let target = replay_request().target();
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(51),
            request: Box::new(replay_request()),
        })
        .expect("replay should start");
    let _ = runtime.drain_events();
    runtime
        .dispatch(AgentCommand::Interrupt {
            agent_id: AgentId::MAIN,
            target: Some(target.clone()),
        })
        .expect("interrupt should replace pending replay facts");
    let events = runtime.drain_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, AgentEventKind::TurnInterrupted));
    assert_eq!(events[0].target, target);
    assert!(runtime.drain_events().is_empty());
}

#[test]
fn replay_keeps_turn_busy_until_terminal_fact_is_drained() {
    let mut runtime =
        super::replay::ReplayAgentRuntime::new(replay_fixture(), RuntimeEventNotifier::default());
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(56),
            request: Box::new(replay_request()),
        })
        .expect("first replay turn should start");
    let busy = runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(57),
            request: Box::new(replay_request()),
        })
        .expect_err("queued terminal fact must keep the turn active");
    assert!(matches!(busy, AgentRuntimeError::Busy));
    assert!(
        runtime
            .drain_events()
            .iter()
            .any(|event| event.kind.is_terminal())
    );
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(58),
            request: Box::new(replay_request()),
        })
        .expect("fixture should be reusable after terminal drain");
    assert!(
        runtime
            .drain_events()
            .iter()
            .any(|event| event.kind.is_terminal())
    );
    runtime.shutdown().expect("replay should shut down cleanly");
}

#[test]
fn replay_shutdown_erases_active_state_and_undelivered_facts() {
    let mut runtime = super::replay::ReplayAgentRuntime::new(
        permission_fixture(),
        RuntimeEventNotifier::default(),
    );
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(59),
            request: Box::new(replay_request()),
        })
        .expect("permission replay should start");
    runtime
        .shutdown()
        .expect("shutdown should dispose active replay state");
    runtime
        .shutdown()
        .expect("repeated replay shutdown should be idempotent");
    assert!(runtime.drain_events().is_empty());
    let error = runtime
        .dispatch(AgentCommand::Interrupt {
            agent_id: AgentId::MAIN,
            target: None,
        })
        .expect_err("disposed replay must reject commands");
    assert!(matches!(error, AgentRuntimeError::Disposed));
}

#[test]
fn replay_projection_uses_the_existing_agent_event_mapper() {
    let (wake_tx, wake_rx) = mpsc::channel();
    let notifier = RuntimeEventNotifier::default();
    let _binding = notifier.bind_callback(move || {
        let _ = wake_tx.send(());
    });
    let mut runtime = super::replay::ReplayAgentRuntime::new(permission_fixture(), notifier);
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(61),
            request: Box::new(replay_request()),
        })
        .expect("replay should start");
    wake_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("initial replay batch should wake after queueing facts");
    let initial_facts = runtime.drain_events();
    let target = replay_request().target();
    runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(target),
            request_id: "replay-permission".to_string(),
            option_id: Some("allow-once".to_string()),
        })
        .expect("permission response should release the replay gate");
    wake_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("post-permission replay batch should wake after queueing facts");
    assert!(
        wake_rx.try_recv().is_err(),
        "one wake is enough for each batch"
    );
    let remaining_facts = runtime.drain_events();
    let projected = initial_facts
        .into_iter()
        .chain(remaining_facts)
        .map(crate::runtime::event_mapping::runtime_event_from_agent_event)
        .collect::<Vec<_>>();
    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::AssistantDelta { content, .. }
            if content == "before approval"
    )));
    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::PermissionRequested { .. }
    )));
    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::ToolActivityStarted { .. }
    )));
    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::MessageFinished { .. }
    )));
}

#[test]
fn replay_failure_fact_uses_the_existing_failed_event_projection() {
    let fixture = super::replay::ReplayFixture::new(vec![AgentEventKind::TurnFailed {
        message: "fixture failure".to_string(),
    }])
    .expect("failure fixture should be valid");
    let mut runtime =
        super::replay::ReplayAgentRuntime::new(fixture, RuntimeEventNotifier::default());
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(62),
            request: Box::new(replay_request()),
        })
        .expect("failure replay should start");
    let projected = runtime
        .drain_events()
        .into_iter()
        .map(crate::runtime::event_mapping::runtime_event_from_agent_event)
        .collect::<Vec<_>>();
    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::Failed { message, .. }
            if message == "fixture failure"
    )));
}

#[test]
fn contract_module_does_not_import_terminal_or_native_worker_types() {
    let source = include_str!("mod.rs");
    for prohibited in [
        ["Conversation", "Worker"].concat(),
        ["LoopEvent", "Waker"].concat(),
    ] {
        assert!(
            !source.contains(&prohibited),
            "Agent contract must not expose {prohibited}"
        );
    }
}
