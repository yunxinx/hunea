use std::{
    ops::{Deref, DerefMut},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use conversation_runtime::RuntimeEventNotifier;
use provider_protocol::{
    ConversationItem, FinishReason, ModelDescriptor, PromptCompletion, PromptRequest,
    ProviderCapabilities, ProviderClient, ProviderError, ProviderFuture, Role, StreamEvent,
    StreamEventSink,
};
use runtime_domain::{
    prompt_assembly::PromptSourceOrigin,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimePermissionOption,
        RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity,
        RuntimeToolActivityStatus, RuntimeToolKind, TranscriptSkillBinding, TranscriptUserMessage,
    },
};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionFuture, ToolExecutorRegistry,
    ToolKind, ToolPermissionPolicy, ToolPermissionPreview, ToolResult,
};

use super::{
    AgentCommand, AgentEvent, AgentEventKind, AgentId, AgentRuntime, AgentRuntimeError,
    AgentTurnId, AgentTurnRequest, NativeAgentRuntime,
};
use crate::runtime::{
    AppRuntimeOptions,
    permission_policy::{
        ApprovalProviderRegistration, InteractiveApprovalProviderFactory, PermissionPolicy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    },
    prompt_assembly::PromptAssemblySessionSnapshot,
};

struct NativeRuntimeFixture {
    runtime: NativeAgentRuntime,
    _approval_registration: ApprovalProviderRegistration,
}

impl Deref for NativeRuntimeFixture {
    type Target = NativeAgentRuntime;

    fn deref(&self) -> &Self::Target {
        &self.runtime
    }
}

impl DerefMut for NativeRuntimeFixture {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.runtime
    }
}

fn permission_policy_fixture(
    event_notifier: RuntimeEventNotifier,
) -> (PermissionPolicy, ApprovalProviderRegistration) {
    let policy = PermissionPolicy::new(event_notifier);
    let registration = policy
        .register(
            "native-test",
            TERMINAL_APPROVAL_PROVIDER_ID,
            std::sync::Arc::new(InteractiveApprovalProviderFactory),
        )
        .expect("native test approval provider should register");
    (policy, registration)
}

fn native_runtime(event_notifier: RuntimeEventNotifier) -> NativeRuntimeFixture {
    let (permission_policy, approval_registration) =
        permission_policy_fixture(event_notifier.clone());
    let runtime = NativeAgentRuntime::new(
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
        None,
        event_notifier,
        crate::runtime::llm_port::LlmPort::new(),
        permission_policy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    )
    .expect("native Agent runtime should initialize");
    NativeRuntimeFixture {
        runtime,
        _approval_registration: approval_registration,
    }
}

struct NativeStreamProvider;

impl ProviderClient for NativeStreamProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let completion = PromptCompletion::new(
                vec![ConversationItem::text(
                    Role::Assistant,
                    "native stream complete",
                )],
                FinishReason::Stop,
                None,
            );
            sink.emit(StreamEvent::TurnStarted);
            sink.emit(StreamEvent::TextDelta("native stream ".to_string()));
            sink.emit(StreamEvent::TextDelta("complete".to_string()));
            sink.emit(StreamEvent::TurnCompleted(completion.clone()));
            Ok(completion)
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

struct NativeStreamFactory;

impl crate::runtime::llm_port::ProviderClientFactory for NativeStreamFactory {
    fn create_client(
        &self,
        _idle_timeout: Duration,
    ) -> Result<std::sync::Arc<dyn ProviderClient>, crate::runtime::llm_port::LlmPortError> {
        Ok(std::sync::Arc::new(NativeStreamProvider))
    }

    fn provider_kind(&self) -> runtime_domain::provider::ProviderKind {
        runtime_domain::provider::ProviderKind::OpenAiCompatible
    }

    fn prompt_cache_policy(&self) -> conversation_runtime::ProviderPromptCachePolicy {
        conversation_runtime::ProviderPromptCachePolicy::Disabled
    }

    fn adapter_kind(&self) -> &'static str {
        "native-stream-fixture"
    }
}

struct NativeFailureProvider;

impl ProviderClient for NativeFailureProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        _sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async {
            Err(ProviderError::Provider {
                status: Some(400),
                message: "https://private.invalid/private-instruction-sentinel".to_string(),
            })
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

struct NativeFailureFactory;

impl crate::runtime::llm_port::ProviderClientFactory for NativeFailureFactory {
    fn create_client(
        &self,
        _idle_timeout: Duration,
    ) -> Result<std::sync::Arc<dyn ProviderClient>, crate::runtime::llm_port::LlmPortError> {
        Ok(std::sync::Arc::new(NativeFailureProvider))
    }

    fn provider_kind(&self) -> runtime_domain::provider::ProviderKind {
        runtime_domain::provider::ProviderKind::OpenAiCompatible
    }

    fn prompt_cache_policy(&self) -> conversation_runtime::ProviderPromptCachePolicy {
        conversation_runtime::ProviderPromptCachePolicy::Disabled
    }

    fn adapter_kind(&self) -> &'static str {
        "native-failure-fixture"
    }
}

struct NativeApprovalProvider {
    call_count: std::sync::atomic::AtomicUsize,
}

impl ProviderClient for NativeApprovalProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let call = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            sink.emit(StreamEvent::TurnStarted);
            let completion = if call == 0 {
                PromptCompletion::new(
                    vec![ConversationItem::assistant_with_tool_calls(
                        "before approval".to_string(),
                        vec![provider_protocol::ToolCall::new(
                            "approval-call",
                            "write",
                            serde_json::json!({"content": "approved"}).to_string(),
                        )],
                    )],
                    FinishReason::ToolCalls,
                    None,
                )
            } else {
                PromptCompletion::new(
                    vec![ConversationItem::text(Role::Assistant, "approved")],
                    FinishReason::Stop,
                    None,
                )
            };
            sink.emit(StreamEvent::TurnCompleted(completion.clone()));
            Ok(completion)
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

struct NativeApprovalFactory;

impl crate::runtime::llm_port::ProviderClientFactory for NativeApprovalFactory {
    fn create_client(
        &self,
        _idle_timeout: Duration,
    ) -> Result<std::sync::Arc<dyn ProviderClient>, crate::runtime::llm_port::LlmPortError> {
        Ok(std::sync::Arc::new(NativeApprovalProvider {
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }))
    }

    fn provider_kind(&self) -> runtime_domain::provider::ProviderKind {
        runtime_domain::provider::ProviderKind::OpenAiCompatible
    }

    fn prompt_cache_policy(&self) -> conversation_runtime::ProviderPromptCachePolicy {
        conversation_runtime::ProviderPromptCachePolicy::Disabled
    }

    fn adapter_kind(&self) -> &'static str {
        "native-approval-fixture"
    }
}

struct NativeAskTool;

impl Tool for NativeAskTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "write complete") })
    }

    fn permission_preview(
        &self,
        _call: &RuntimeToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        Some(ToolPermissionPreview {
            path: "fixture.txt".to_string(),
            old_text: None,
            new_text: "approved".to_string(),
            is_truncated: false,
            snapshot: None,
        })
    }
}

fn failing_turn_request() -> AgentTurnRequest {
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "openai",
        "gpt-4o-mini",
        ConversationItem::text(Role::User, "hello"),
    ))
}

fn replay_request() -> AgentTurnRequest {
    AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "replay",
        "fixture-model",
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
            "model",
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
    let runtime_contract: &mut dyn AgentRuntime = &mut *runtime;

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
fn native_runtime_streams_through_the_llm_port_factory() {
    let llm_port = crate::runtime::llm_port::LlmPort::new();
    let _registration = llm_port
        .register(
            "native-stream-test",
            "fixture",
            std::sync::Arc::new(NativeStreamFactory),
        )
        .expect("fixture provider should register");
    let notifier = RuntimeEventNotifier::default();
    let (permission_policy, _approval_registration) = permission_policy_fixture(notifier.clone());
    let mut runtime = NativeAgentRuntime::new(
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
        None,
        notifier,
        llm_port,
        permission_policy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    )
    .expect("native Agent runtime should initialize");
    let request = AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "fixture",
        "fixture-model",
        ConversationItem::text(Role::User, "hello"),
    ));
    assert_eq!(
        request.target(),
        RuntimeTarget::provider("fixture", "fixture-model")
    );

    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(18),
            request: Box::new(request),
        })
        .expect("native turn should start through LlmPort");
    let events = collect_until_terminal(&mut runtime);

    assert!(events.iter().any(|event| matches!(
        &event.kind,
        AgentEventKind::AssistantDelta { content } if content == "native stream " || content == "complete"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        AgentEventKind::TurnFinished { response, .. }
            if response == &ConversationResponse::assistant_text("native stream complete")
    )));
    runtime
        .shutdown()
        .expect("native runtime should shut down cleanly");
}

#[test]
fn native_runtime_routes_tool_approval_through_the_live_permission_turn() {
    let llm_port = crate::runtime::llm_port::LlmPort::new();
    let _registration = llm_port
        .register(
            "native-approval-test",
            "fixture",
            std::sync::Arc::new(NativeApprovalFactory),
        )
        .expect("approval fixture provider should register");
    let notifier = RuntimeEventNotifier::default();
    let (permission_policy, _approval_registration) = permission_policy_fixture(notifier.clone());
    let mut tools = ToolExecutorRegistry::new();
    tools.insert(NativeAskTool);
    let mut runtime = NativeAgentRuntime::new(
        &AppRuntimeOptions {
            runtime_request_policy: runtime_domain::request_policy::RuntimeRequestPolicy::new(
                0,
                Vec::new(),
                1,
            ),
            ..AppRuntimeOptions::default()
        },
        tools,
        Vec::new(),
        PromptAssemblySessionSnapshot::default(),
        None,
        notifier,
        llm_port,
        permission_policy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    )
    .expect("native Agent runtime should initialize");
    let request = AgentTurnRequest::from_conversation_request(ConversationTurnRequest::new(
        "fixture",
        "fixture-model",
        ConversationItem::text(Role::User, "approve this tool"),
    ));
    let target = request.target();
    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(19),
            request: Box::new(request),
        })
        .expect("native approval turn should start");

    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        events.extend(runtime.drain_events());
        if events
            .iter()
            .any(|event| matches!(&event.kind, AgentEventKind::PermissionRequested { .. }))
            || events.iter().any(|event| event.kind.is_terminal())
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let permission_index = events
        .iter()
        .position(|event| matches!(&event.kind, AgentEventKind::PermissionRequested { .. }))
        .unwrap_or_else(|| panic!("native worker should publish an approval request: {events:?}"));
    assert!(
        events[..permission_index]
            .iter()
            .any(|event| matches!(event.kind, AgentEventKind::ToolActivityStarted { .. }))
    );
    let request_id = match &events[permission_index].kind {
        AgentEventKind::PermissionRequested { request } => request.request_id.clone(),
        _ => unreachable!(),
    };
    runtime
        .dispatch(AgentCommand::RespondPermission {
            agent_id: AgentId::MAIN,
            target: Some(target),
            request_id,
            option_id: Some("allow_once".to_string()),
        })
        .expect("approval response should route to the active turn");

    let remaining = collect_until_terminal(&mut runtime);
    assert!(
        remaining
            .iter()
            .any(|event| matches!(event.kind, AgentEventKind::TurnFinished { .. }))
    );
    runtime
        .shutdown()
        .expect("native approval runtime should shut down cleanly");
}

#[test]
fn provider_failure_is_redacted_before_runtime_event_projection() {
    let sentinel = "https://private.invalid/private-instruction-sentinel";
    let llm_port = crate::runtime::llm_port::LlmPort::new();
    let _registration = llm_port
        .register(
            "native-failure-test",
            "fixture",
            std::sync::Arc::new(NativeFailureFactory),
        )
        .expect("fixture provider should register");
    let notifier = RuntimeEventNotifier::default();
    let (permission_policy, _approval_registration) = permission_policy_fixture(notifier.clone());
    let mut runtime = NativeAgentRuntime::new(
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
        None,
        notifier,
        llm_port,
        permission_policy,
        TERMINAL_APPROVAL_PROVIDER_ID,
    )
    .expect("native Agent runtime should initialize");

    runtime
        .dispatch(AgentCommand::SubmitTurn {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(19),
            request: Box::new(AgentTurnRequest::from_conversation_request(
                ConversationTurnRequest::new(
                    "fixture",
                    "fixture-model",
                    ConversationItem::text(Role::User, "hello"),
                ),
            )),
        })
        .expect("native turn should start through LlmPort");
    let projected = collect_until_terminal(&mut runtime)
        .into_iter()
        .map(crate::runtime::event_mapping::runtime_event_from_agent_event)
        .collect::<Vec<_>>();

    assert!(projected.iter().any(|event| matches!(
        event,
        runtime_domain::session::RuntimeEvent::Failed { message, .. }
            if message == "provider request failed"
    )));
    assert!(
        projected
            .iter()
            .all(|event| !format!("{event:?}").contains(sentinel)),
        "provider source details must not reach RuntimeEvent"
    );
    runtime
        .shutdown()
        .expect("native runtime should shut down cleanly");
}

#[test]
fn shared_identity_and_terminal_contract_runs_for_native_and_replay() {
    let mut native = native_runtime(RuntimeEventNotifier::default());
    assert_shared_identity_and_terminal(
        &mut *native,
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
    assert_shared_lifecycle_contract(&mut *native);

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
    assert_shared_busy_and_interrupt_contract(&mut *native, native_target, failing_turn_request());

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
