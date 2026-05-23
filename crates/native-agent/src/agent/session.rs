use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use mo_ai_core::{Message, ToolCall, ToolResult};
use tokio_util::sync::CancellationToken;

use mo_core::{
    request_policy::RuntimeRequestPolicy,
    session::{NativeAgentEvent, RuntimeTarget},
};
use mo_tools::{SharedToolPermissionHandler, ToolExecutorRegistry};

use super::{
    NativeAgentError, NativePermissionBroker, response::NativeAgentProgress,
    turn::send_execution_loop_with_cancellation_and_progress,
};
use crate::NativeAgentExecutionRequest;

const TIMEOUT_REPAIR_GRACE: Duration = Duration::from_secs(2);
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";
const TOOL_EXECUTION_TIMED_OUT: &str = "Tool execution timed out";

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeAgentWorkerEvent {
    Progress(NativeAgentEvent),
    Session(NativeAgentSessionEvent),
    Finished {
        response: mo_core::session::NativeAgentResponse,
        metrics: Option<crate::NativeLlmPerformanceMetrics>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeAgentSessionEvent {
    ProviderTurnStarted,
    ProviderContextMessage { message: Message },
}

impl NativeAgentWorkerEvent {
    fn progress(event: NativeAgentEvent) -> Self {
        Self::Progress(event)
    }
}

/// `NativeAgentRuntimeState` 管理内置 native agent 请求的后台 worker 与取消状态。
#[derive(Default)]
pub struct NativeAgentRuntimeState {
    receiver: Option<Receiver<NativeAgentWorkerEvent>>,
    pub cancellation: Option<CancellationToken>,
    pub target: Option<RuntimeTarget>,
    permission_broker: Option<NativePermissionBroker>,
    provider_turn_started: bool,
    session_messages: Vec<Message>,
}

impl NativeAgentRuntimeState {
    pub fn start(
        &mut self,
        request: NativeAgentExecutionRequest,
        executor: ToolExecutorRegistry,
        request_policy: RuntimeRequestPolicy,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        let target = request.target();
        let permission_broker = NativePermissionBroker::default();
        let thread_permission_broker = permission_broker.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run_native_agent_worker(
                        request,
                        executor,
                        request_policy,
                        thread_cancellation,
                        thread_permission_broker,
                        sender,
                    ));
                }
                Err(error) => {
                    let _ =
                        sender.send(NativeAgentWorkerEvent::progress(NativeAgentEvent::Failed {
                            message: format!("start agent runtime: {error}"),
                        }));
                }
            }
        });
        self.receiver = Some(receiver);
        self.cancellation = Some(cancellation);
        self.target = Some(target);
        self.permission_broker = Some(permission_broker);
        self.provider_turn_started = false;
        self.session_messages.clear();
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn reset_after_clear(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
        }
        self.receiver = None;
        self.target = None;
        self.provider_turn_started = false;
        self.session_messages.clear();
    }

    pub fn interrupt(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
        }
        true
    }

    pub fn respond_permission(
        &mut self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let Some(permission_broker) = self.permission_broker.as_ref() else {
            return Err("Native agent is not waiting for permission".to_string());
        };
        permission_broker.respond_permission(request_id, option_id)
    }

    pub fn current_target(&self) -> Option<&RuntimeTarget> {
        self.target.as_ref()
    }

    pub fn take_provider_turn_started(&mut self) -> bool {
        std::mem::take(&mut self.provider_turn_started)
    }

    pub fn take_session_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.session_messages)
    }

    pub fn try_recv_event(&mut self) -> Option<NativeAgentEvent> {
        loop {
            let event = match self.receiver.as_ref()?.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.cancellation = None;
                    self.target = None;
                    if let Some(permission_broker) = self.permission_broker.take() {
                        permission_broker.cancel_all();
                    }
                    return Some(NativeAgentEvent::Failed {
                        message: "agent request stopped before completion".to_string(),
                    });
                }
            };

            match event {
                NativeAgentWorkerEvent::Progress(event) => {
                    if event.is_terminal() {
                        self.receiver = None;
                        self.cancellation = None;
                        self.target = None;
                        if let Some(permission_broker) = self.permission_broker.take() {
                            permission_broker.cancel_all();
                        }
                    }
                    return Some(event);
                }
                NativeAgentWorkerEvent::Session(event) => {
                    self.apply_session_event(event);
                }
                NativeAgentWorkerEvent::Finished { response, metrics } => {
                    self.receiver = None;
                    self.cancellation = None;
                    self.target = None;
                    if let Some(permission_broker) = self.permission_broker.take() {
                        permission_broker.cancel_all();
                    }
                    return Some(NativeAgentEvent::Finished { response, metrics });
                }
            }
        }
    }

    fn apply_session_event(&mut self, event: NativeAgentSessionEvent) {
        match event {
            NativeAgentSessionEvent::ProviderTurnStarted => {
                self.provider_turn_started = true;
            }
            NativeAgentSessionEvent::ProviderContextMessage { message } => {
                self.session_messages.push(message);
            }
        }
    }
}

async fn run_native_agent_worker(
    request: NativeAgentExecutionRequest,
    executor: ToolExecutorRegistry,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    permission_broker: NativePermissionBroker,
    sender: mpsc::Sender<NativeAgentWorkerEvent>,
) {
    let provider_context_messages_started = Arc::new(AtomicBool::new(false));
    let provider_context_repair_ledger =
        Arc::new(Mutex::new(ProviderContextRepairLedger::default()));
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let attempt_provider_context_messages_started =
            Arc::clone(&provider_context_messages_started);
        let attempt_provider_context_repair_ledger = Arc::clone(&provider_context_repair_ledger);
        let attempt_cancellation = cancellation.child_token();
        let permission_handler: SharedToolPermissionHandler = std::sync::Arc::new(
            permission_broker.handler(progress_sender_to_permission_sender(sender.clone())),
        );
        let attempt_result = run_with_soft_timeout(
            &cancellation,
            &attempt_cancellation,
            request_policy.timeout(),
            TIMEOUT_REPAIR_GRACE,
            send_execution_loop_with_cancellation_and_progress(
                &request,
                executor.clone(),
                &attempt_cancellation,
                request_policy.tool_max_turns(),
                Some(permission_handler),
                move |progress| {
                    let event = native_agent_worker_event_from_progress(progress);
                    if let NativeAgentWorkerEvent::Session(
                        NativeAgentSessionEvent::ProviderContextMessage { message },
                    ) = &event
                    {
                        attempt_provider_context_messages_started.store(true, Ordering::Relaxed);
                        attempt_provider_context_repair_ledger
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .observe(message);
                    }
                    let _ = progress_sender.send(event);
                },
            ),
        )
        .await;
        let can_retry_from_original_request =
            !provider_context_messages_started.load(Ordering::Relaxed);

        match attempt_result {
            NativeAgentAttemptOutcome::TimedOut(Err(_))
            | NativeAgentAttemptOutcome::TimedOutAfterGrace
                if attempt < request_policy.attempts() && can_retry_from_original_request =>
            {
                permission_broker.cancel_all();
                if retry_native_agent_after_attempt(
                    attempt,
                    &request_policy,
                    &cancellation,
                    &sender,
                )
                .await
                {
                    return;
                }
            }
            NativeAgentAttemptOutcome::TimedOut(Err(_))
            | NativeAgentAttemptOutcome::TimedOutAfterGrace => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_TIMED_OUT,
                );
                let _ = sender.send(NativeAgentWorkerEvent::progress(NativeAgentEvent::Failed {
                    message: format!(
                        "Agent request timed out after {}s",
                        request_policy.timeout().as_secs()
                    ),
                }));
                return;
            }
            NativeAgentAttemptOutcome::Completed(Ok(completion))
            | NativeAgentAttemptOutcome::TimedOut(Ok(completion)) => {
                permission_broker.cancel_all();
                let _ = sender.send(NativeAgentWorkerEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                });
                return;
            }
            NativeAgentAttemptOutcome::Completed(Err(NativeAgentError::Cancelled)) => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(NativeAgentWorkerEvent::progress(
                    NativeAgentEvent::Interrupted,
                ));
                return;
            }
            NativeAgentAttemptOutcome::CancelledAfterGrace => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(NativeAgentWorkerEvent::progress(
                    NativeAgentEvent::Interrupted,
                ));
                return;
            }
            NativeAgentAttemptOutcome::Completed(Err(_error))
                if attempt < request_policy.attempts() && can_retry_from_original_request =>
            {
                permission_broker.cancel_all();
                if retry_native_agent_after_attempt(
                    attempt,
                    &request_policy,
                    &cancellation,
                    &sender,
                )
                .await
                {
                    return;
                }
            }
            NativeAgentAttemptOutcome::Completed(Err(error)) => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(NativeAgentWorkerEvent::progress(NativeAgentEvent::Failed {
                    message: error.to_string(),
                }));
                return;
            }
        }
    }
}

enum NativeAgentAttemptOutcome<T> {
    Completed(T),
    TimedOut(T),
    TimedOutAfterGrace,
    CancelledAfterGrace,
}

async fn run_with_soft_timeout<T>(
    parent_cancellation: &CancellationToken,
    attempt_cancellation: &CancellationToken,
    timeout: Duration,
    repair_grace: Duration,
    future: impl Future<Output = T>,
) -> NativeAgentAttemptOutcome<T> {
    let timeout = tokio::time::sleep(timeout);
    tokio::pin!(timeout);
    tokio::pin!(future);

    tokio::select! {
        output = &mut future => NativeAgentAttemptOutcome::Completed(output),
        _ = parent_cancellation.cancelled() => {
            attempt_cancellation.cancel();
            let repair_grace = tokio::time::sleep(repair_grace);
            tokio::pin!(repair_grace);
            tokio::select! {
                output = &mut future => NativeAgentAttemptOutcome::Completed(output),
                _ = &mut repair_grace => NativeAgentAttemptOutcome::CancelledAfterGrace,
            }
        }
        _ = &mut timeout => {
            attempt_cancellation.cancel();
            let repair_grace = tokio::time::sleep(repair_grace);
            tokio::pin!(repair_grace);
            tokio::select! {
                output = &mut future => NativeAgentAttemptOutcome::TimedOut(output),
                _ = parent_cancellation.cancelled() => NativeAgentAttemptOutcome::CancelledAfterGrace,
                _ = &mut repair_grace => NativeAgentAttemptOutcome::TimedOutAfterGrace,
            }
        }
    }
}

#[derive(Debug, Default)]
struct ProviderContextRepairLedger {
    unresolved_tool_calls: Vec<ToolCall>,
}

impl ProviderContextRepairLedger {
    fn observe(&mut self, message: &Message) {
        self.unresolved_tool_calls.extend(message.tool_calls());

        if let Some(tool_result) = message.first_tool_result() {
            self.resolve_tool_result(&tool_result.call_id);
        }
    }

    fn resolve_tool_result(&mut self, call_id: &str) {
        let Some(index) = self
            .unresolved_tool_calls
            .iter()
            .position(|call| call.call_id == call_id)
        else {
            return;
        };

        self.unresolved_tool_calls.remove(index);
    }

    fn take_repair_messages(&mut self, content: &'static str) -> Vec<Message> {
        std::mem::take(&mut self.unresolved_tool_calls)
            .into_iter()
            .map(|call| {
                Message::tool_result(ToolResult::error(call.call_id, call.name, content, None))
            })
            .collect()
    }
}

fn emit_provider_context_repair_messages(
    ledger: &Mutex<ProviderContextRepairLedger>,
    sender: &mpsc::Sender<NativeAgentWorkerEvent>,
    content: &'static str,
) {
    let repair_messages = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_repair_messages(content);

    for message in repair_messages {
        let _ = sender.send(NativeAgentWorkerEvent::Session(
            NativeAgentSessionEvent::ProviderContextMessage { message },
        ));
    }
}

fn progress_sender_to_permission_sender(
    sender: mpsc::Sender<NativeAgentWorkerEvent>,
) -> mpsc::Sender<NativeAgentEvent> {
    let (permission_sender, permission_receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(event) = permission_receiver.recv() {
            let _ = sender.send(NativeAgentWorkerEvent::progress(event));
        }
    });
    permission_sender
}

fn native_agent_worker_event_from_progress(
    progress: NativeAgentProgress,
) -> NativeAgentWorkerEvent {
    match progress {
        NativeAgentProgress::ProviderTurnStarted => {
            NativeAgentWorkerEvent::Session(NativeAgentSessionEvent::ProviderTurnStarted)
        }
        NativeAgentProgress::ProviderContextMessage { message } => {
            NativeAgentWorkerEvent::Session(NativeAgentSessionEvent::ProviderContextMessage {
                message,
            })
        }
        NativeAgentProgress::OutputTokens { total_tokens } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::OutputTokenEstimate { total_tokens })
        }
        NativeAgentProgress::InputTokens { total_tokens } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::InputTokenEstimate { total_tokens })
        }
        NativeAgentProgress::Thinking { is_thinking } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::Thinking { is_thinking })
        }
        NativeAgentProgress::AssistantDelta { content } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::AssistantDelta { content })
        }
        NativeAgentProgress::ReasoningDelta { content } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::ReasoningDelta { content })
        }
        NativeAgentProgress::ToolActivityStarted { activity } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::ToolActivityStarted { activity })
        }
        NativeAgentProgress::ToolActivityUpdated { update } => {
            NativeAgentWorkerEvent::progress(NativeAgentEvent::ToolActivityUpdated { update })
        }
    }
}

async fn retry_native_agent_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<NativeAgentWorkerEvent>,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(NativeAgentWorkerEvent::progress(
        NativeAgentEvent::Retrying {
            message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
        },
    ));
    tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = sender.send(NativeAgentWorkerEvent::progress(NativeAgentEvent::Interrupted));
            true
        }
        _ = tokio::time::sleep(request_policy.delay_for_retry(retry)) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use mo_ai_core::{Message, MessageRole, ToolCall, ToolResult};
    use mo_core::{request_policy::RuntimeRequestPolicy, session::RuntimeTarget};
    use mo_tools::ToolExecutorRegistry;
    use tokio_util::sync::CancellationToken;

    use super::{
        NativeAgentEvent, NativeAgentRuntimeState, NativeAgentSessionEvent, NativeAgentWorkerEvent,
        NativePermissionBroker,
    };
    use crate::{NativeAgentExecutionRequest, NativeAgentResponse, ProviderKind};

    #[test]
    fn native_agent_runtime_clears_receiver_after_terminal_event() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(NativeAgentWorkerEvent::progress(
                NativeAgentEvent::Interrupted,
            ))
            .expect("send terminal event");
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Interrupted)
        );
        assert!(!runtime.is_running());
        assert!(runtime.current_target().is_none());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_retry_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(NativeAgentWorkerEvent::progress(
                NativeAgentEvent::Retrying {
                    message: "Reconnecting... 1/3".to_string(),
                },
            ))
            .expect("retry event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
        );
        assert!(runtime.is_running());

        sender
            .send(NativeAgentWorkerEvent::Finished {
                response: NativeAgentResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
            .expect("finish event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Finished {
                response: NativeAgentResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
        );
        assert!(!runtime.is_running());
        assert!(runtime.take_session_messages().is_empty());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_token_estimate_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(NativeAgentWorkerEvent::progress(
                NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 },
            ))
            .expect("token estimate event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_text_delta_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(NativeAgentWorkerEvent::progress(
                NativeAgentEvent::AssistantDelta {
                    content: "partial".to_string(),
                },
            ))
            .expect("assistant delta event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::AssistantDelta {
                content: "partial".to_string(),
            })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn native_agent_runtime_buffers_session_events_without_ui_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };
        let message = Message::text(MessageRole::Assistant, "stored");

        sender
            .send(NativeAgentWorkerEvent::Session(
                NativeAgentSessionEvent::ProviderTurnStarted,
            ))
            .expect("provider event should be queued");
        sender
            .send(NativeAgentWorkerEvent::Session(
                NativeAgentSessionEvent::ProviderContextMessage {
                    message: message.clone(),
                },
            ))
            .expect("message event should be queued");

        assert_eq!(runtime.try_recv_event(), None);
        assert!(runtime.take_provider_turn_started());
        assert_eq!(runtime.take_session_messages(), vec![message]);
        assert!(runtime.is_running());
    }

    #[test]
    fn native_agent_interrupt_keeps_receiver_until_worker_terminal_event() {
        let (_sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        assert!(runtime.interrupt());
        assert!(runtime.is_running());
        assert!(runtime.current_target().is_some());
    }

    #[tokio::test]
    async fn soft_timeout_cancels_child_and_waits_for_repair() {
        let parent_cancellation = CancellationToken::new();
        let attempt_cancellation = parent_cancellation.child_token();
        let future_cancellation = attempt_cancellation.clone();
        let repair_observed = Arc::new(AtomicBool::new(false));
        let future_repair_observed = Arc::clone(&repair_observed);

        let outcome = super::run_with_soft_timeout(
            &parent_cancellation,
            &attempt_cancellation,
            Duration::from_millis(10),
            Duration::from_millis(10),
            async move {
                future_cancellation.cancelled().await;
                future_repair_observed.store(true, Ordering::Relaxed);
                "repaired"
            },
        )
        .await;

        match outcome {
            super::NativeAgentAttemptOutcome::TimedOut(value) => assert_eq!(value, "repaired"),
            super::NativeAgentAttemptOutcome::Completed(_) => {
                panic!("timeout should mark the attempt as timed out")
            }
            super::NativeAgentAttemptOutcome::TimedOutAfterGrace
            | super::NativeAgentAttemptOutcome::CancelledAfterGrace => {
                panic!("future should finish inside the repair grace")
            }
        }
        assert!(repair_observed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn soft_timeout_stops_waiting_after_repair_grace() {
        let parent_cancellation = CancellationToken::new();
        let attempt_cancellation = parent_cancellation.child_token();

        let outcome = super::run_with_soft_timeout(
            &parent_cancellation,
            &attempt_cancellation,
            Duration::from_millis(5),
            Duration::from_millis(5),
            async {
                std::future::pending::<()>().await;
                "never"
            },
        )
        .await;

        assert!(matches!(
            outcome,
            super::NativeAgentAttemptOutcome::TimedOutAfterGrace
        ));
        assert!(attempt_cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn parent_cancellation_stops_waiting_after_repair_grace() {
        let parent_cancellation = CancellationToken::new();
        let attempt_cancellation = parent_cancellation.child_token();
        parent_cancellation.cancel();

        let outcome = super::run_with_soft_timeout(
            &parent_cancellation,
            &attempt_cancellation,
            Duration::from_secs(1),
            Duration::from_millis(5),
            async {
                std::future::pending::<()>().await;
                "never"
            },
        )
        .await;

        assert!(matches!(
            outcome,
            super::NativeAgentAttemptOutcome::CancelledAfterGrace
        ));
        assert!(attempt_cancellation.is_cancelled());
    }

    #[test]
    fn provider_context_repair_messages_fill_unresolved_tool_calls() {
        let mut ledger = super::ProviderContextRepairLedger::default();
        let first_call = ToolCall::new("call-1", "read", serde_json::json!({}));
        let second_call = ToolCall::new("call-2", "search", serde_json::json!({}));

        ledger.observe(&Message::assistant_with_tool_calls(
            String::new(),
            vec![first_call, second_call],
        ));
        ledger.observe(&Message::tool_result(ToolResult::success(
            "call-1", "read", "done", None,
        )));

        let repair_messages = ledger.take_repair_messages(super::TOOL_EXECUTION_TIMED_OUT);

        assert_eq!(repair_messages.len(), 1);
        let repair_result = repair_messages[0]
            .first_tool_result()
            .expect("repair message should contain a tool result");
        assert_eq!(repair_result.call_id, "call-2");
        assert_eq!(repair_result.name, "search");
        assert!(repair_result.is_error);
        assert_eq!(repair_result.content, super::TOOL_EXECUTION_TIMED_OUT);
        assert!(
            ledger
                .take_repair_messages(super::TOOL_EXECUTION_TIMED_OUT)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn native_agent_worker_reports_interrupted_when_pre_cancelled() {
        let request = NativeAgentExecutionRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![Message::text(MessageRole::User, "hello")],
        );
        let executor = ToolExecutorRegistry::new();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (sender, receiver) = mpsc::channel();

        super::run_native_agent_worker(
            request,
            executor,
            RuntimeRequestPolicy::default(),
            cancellation,
            NativePermissionBroker::default(),
            sender,
        )
        .await;

        assert_eq!(
            receiver.recv().expect("worker should emit an event"),
            NativeAgentWorkerEvent::progress(NativeAgentEvent::Interrupted)
        );
    }
}
