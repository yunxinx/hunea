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

use provider_protocol::{Message, ToolCall, ToolResult};
use tokio_util::sync::CancellationToken;

use runtime_domain::{
    request_policy::RuntimeRequestPolicy,
    session::{ConversationEvent, RuntimeTarget},
};
use tool_runtime::{SharedToolPermissionHandler, ToolExecutorRegistry};

use super::{
    ConversationPermissionBroker, TurnExecutionError, response::ConversationProgress,
    turn::run_prepared_conversation_with_progress,
};
use crate::PreparedConversationRequest;

const TIMEOUT_REPAIR_GRACE: Duration = Duration::from_secs(2);
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";
const TOOL_EXECUTION_TIMED_OUT: &str = "Tool execution timed out";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConversationWorkerEvent {
    Progress(ConversationEvent),
    Session(ConversationDelta),
    Finished {
        response: runtime_domain::session::ConversationResponse,
        metrics: Option<crate::ProviderRequestMetrics>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConversationDelta {
    ProviderTurnStarted,
    ProviderContextMessage { message: Message },
}

impl ConversationWorkerEvent {
    fn progress(event: ConversationEvent) -> Self {
        Self::Progress(event)
    }
}

/// `ConversationWorker` 管理对话请求的后台 worker 与取消状态。
#[derive(Default)]
pub struct ConversationWorker {
    receiver: Option<Receiver<ConversationWorkerEvent>>,
    pub cancellation: Option<CancellationToken>,
    pub target: Option<RuntimeTarget>,
    permission_broker: Option<ConversationPermissionBroker>,
    provider_turn_started: bool,
    session_messages: Vec<Message>,
}

impl ConversationWorker {
    pub fn start(
        &mut self,
        request: PreparedConversationRequest,
        executor: ToolExecutorRegistry,
        request_policy: RuntimeRequestPolicy,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        let target = request.target();
        let permission_broker = ConversationPermissionBroker::default();
        let thread_permission_broker = permission_broker.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run_conversation_worker(
                        request,
                        executor,
                        request_policy,
                        thread_cancellation,
                        thread_permission_broker,
                        sender,
                    ));
                }
                Err(error) => {
                    let _ = sender.send(ConversationWorkerEvent::progress(
                        ConversationEvent::Failed {
                            message: format!("start conversation runtime: {error}"),
                        },
                    ));
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
            return Err("Conversation worker is not waiting for permission".to_string());
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

    pub fn try_recv_event(&mut self) -> Option<ConversationEvent> {
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
                    return Some(ConversationEvent::Failed {
                        message: "conversation request stopped before completion".to_string(),
                    });
                }
            };

            match event {
                ConversationWorkerEvent::Progress(event) => {
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
                ConversationWorkerEvent::Session(event) => {
                    self.apply_session_event(event);
                }
                ConversationWorkerEvent::Finished { response, metrics } => {
                    self.receiver = None;
                    self.cancellation = None;
                    self.target = None;
                    if let Some(permission_broker) = self.permission_broker.take() {
                        permission_broker.cancel_all();
                    }
                    return Some(ConversationEvent::Finished { response, metrics });
                }
            }
        }
    }

    fn apply_session_event(&mut self, event: ConversationDelta) {
        match event {
            ConversationDelta::ProviderTurnStarted => {
                self.provider_turn_started = true;
            }
            ConversationDelta::ProviderContextMessage { message } => {
                self.session_messages.push(message);
            }
        }
    }
}

async fn run_conversation_worker(
    request: PreparedConversationRequest,
    executor: ToolExecutorRegistry,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    permission_broker: ConversationPermissionBroker,
    sender: mpsc::Sender<ConversationWorkerEvent>,
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
            run_prepared_conversation_with_progress(
                &request,
                executor.clone(),
                &attempt_cancellation,
                request_policy.tool_max_turns(),
                Some(permission_handler),
                move |progress| {
                    let event = conversation_worker_event_from_progress(progress);
                    if let ConversationWorkerEvent::Session(
                        ConversationDelta::ProviderContextMessage { message },
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
            TurnAttemptOutcome::TimedOut(Err(_)) | TurnAttemptOutcome::TimedOutAfterGrace
                if attempt < request_policy.attempts() && can_retry_from_original_request =>
            {
                permission_broker.cancel_all();
                if retry_conversation_after_attempt(
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
            TurnAttemptOutcome::TimedOut(Err(_)) | TurnAttemptOutcome::TimedOutAfterGrace => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_TIMED_OUT,
                );
                let _ = sender.send(ConversationWorkerEvent::progress(
                    ConversationEvent::Failed {
                        message: format!(
                            "Conversation request timed out after {}s",
                            request_policy.timeout().as_secs()
                        ),
                    },
                ));
                return;
            }
            TurnAttemptOutcome::Completed(Ok(completion))
            | TurnAttemptOutcome::TimedOut(Ok(completion)) => {
                permission_broker.cancel_all();
                let _ = sender.send(ConversationWorkerEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                });
                return;
            }
            TurnAttemptOutcome::Completed(Err(TurnExecutionError::Cancelled)) => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(ConversationWorkerEvent::progress(
                    ConversationEvent::Interrupted,
                ));
                return;
            }
            TurnAttemptOutcome::CancelledAfterGrace => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(ConversationWorkerEvent::progress(
                    ConversationEvent::Interrupted,
                ));
                return;
            }
            TurnAttemptOutcome::Completed(Err(_error))
                if attempt < request_policy.attempts() && can_retry_from_original_request =>
            {
                permission_broker.cancel_all();
                if retry_conversation_after_attempt(
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
            TurnAttemptOutcome::Completed(Err(error)) => {
                permission_broker.cancel_all();
                emit_provider_context_repair_messages(
                    provider_context_repair_ledger.as_ref(),
                    &sender,
                    TOOL_EXECUTION_INTERRUPTED,
                );
                let _ = sender.send(ConversationWorkerEvent::progress(
                    ConversationEvent::Failed {
                        message: error.to_string(),
                    },
                ));
                return;
            }
        }
    }
}

enum TurnAttemptOutcome<T> {
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
) -> TurnAttemptOutcome<T> {
    let timeout = tokio::time::sleep(timeout);
    tokio::pin!(timeout);
    tokio::pin!(future);

    tokio::select! {
        output = &mut future => TurnAttemptOutcome::Completed(output),
        _ = parent_cancellation.cancelled() => {
            attempt_cancellation.cancel();
            let repair_grace = tokio::time::sleep(repair_grace);
            tokio::pin!(repair_grace);
            tokio::select! {
                output = &mut future => TurnAttemptOutcome::Completed(output),
                _ = &mut repair_grace => TurnAttemptOutcome::CancelledAfterGrace,
            }
        }
        _ = &mut timeout => {
            attempt_cancellation.cancel();
            let repair_grace = tokio::time::sleep(repair_grace);
            tokio::pin!(repair_grace);
            tokio::select! {
                output = &mut future => TurnAttemptOutcome::TimedOut(output),
                _ = parent_cancellation.cancelled() => TurnAttemptOutcome::CancelledAfterGrace,
                _ = &mut repair_grace => TurnAttemptOutcome::TimedOutAfterGrace,
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
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    content: &'static str,
) {
    let repair_messages = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_repair_messages(content);

    for message in repair_messages {
        let _ = sender.send(ConversationWorkerEvent::Session(
            ConversationDelta::ProviderContextMessage { message },
        ));
    }
}

fn progress_sender_to_permission_sender(
    sender: mpsc::Sender<ConversationWorkerEvent>,
) -> mpsc::Sender<ConversationEvent> {
    let (permission_sender, permission_receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(event) = permission_receiver.recv() {
            let _ = sender.send(ConversationWorkerEvent::progress(event));
        }
    });
    permission_sender
}

fn conversation_worker_event_from_progress(
    progress: ConversationProgress,
) -> ConversationWorkerEvent {
    match progress {
        ConversationProgress::ProviderTurnStarted => {
            ConversationWorkerEvent::Session(ConversationDelta::ProviderTurnStarted)
        }
        ConversationProgress::ProviderContextMessage { message } => {
            ConversationWorkerEvent::Session(ConversationDelta::ProviderContextMessage { message })
        }
        ConversationProgress::OutputTokens { total_tokens } => {
            ConversationWorkerEvent::progress(ConversationEvent::OutputTokenEstimate {
                total_tokens,
            })
        }
        ConversationProgress::InputTokens { total_tokens } => {
            ConversationWorkerEvent::progress(ConversationEvent::InputTokenEstimate {
                total_tokens,
            })
        }
        ConversationProgress::Thinking { is_thinking } => {
            ConversationWorkerEvent::progress(ConversationEvent::Thinking { is_thinking })
        }
        ConversationProgress::AssistantDelta { content } => {
            ConversationWorkerEvent::progress(ConversationEvent::AssistantDelta { content })
        }
        ConversationProgress::ReasoningDelta { content } => {
            ConversationWorkerEvent::progress(ConversationEvent::ReasoningDelta { content })
        }
        ConversationProgress::ToolActivityStarted { activity } => {
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityStarted { activity })
        }
        ConversationProgress::ToolActivityUpdated { update } => {
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityUpdated { update })
        }
    }
}

async fn retry_conversation_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(ConversationWorkerEvent::progress(
        ConversationEvent::Retrying {
            message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
        },
    ));
    tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = sender.send(ConversationWorkerEvent::progress(ConversationEvent::Interrupted));
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

    use provider_protocol::{Message, MessageRole, ToolCall, ToolResult};
    use runtime_domain::{request_policy::RuntimeRequestPolicy, session::RuntimeTarget};
    use tokio_util::sync::CancellationToken;
    use tool_runtime::ToolExecutorRegistry;

    use super::{
        ConversationDelta, ConversationEvent, ConversationPermissionBroker, ConversationWorker,
        ConversationWorkerEvent,
    };
    use crate::{ConversationResponse, PreparedConversationRequest, ProviderKind};

    #[test]
    fn conversation_runtime_clears_receiver_after_terminal_event() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(ConversationWorkerEvent::progress(
                ConversationEvent::Interrupted,
            ))
            .expect("send terminal event");
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::Interrupted)
        );
        assert!(!runtime.is_running());
        assert!(runtime.current_target().is_none());
    }

    #[test]
    fn conversation_runtime_keeps_receiver_after_retry_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(ConversationWorkerEvent::progress(
                ConversationEvent::Retrying {
                    message: "Reconnecting... 1/3".to_string(),
                },
            ))
            .expect("retry event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
        );
        assert!(runtime.is_running());

        sender
            .send(ConversationWorkerEvent::Finished {
                response: ConversationResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
            .expect("finish event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::Finished {
                response: ConversationResponse {
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
    fn conversation_runtime_keeps_receiver_after_token_estimate_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(ConversationWorkerEvent::progress(
                ConversationEvent::OutputTokenEstimate { total_tokens: 12 },
            ))
            .expect("token estimate event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::OutputTokenEstimate { total_tokens: 12 })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn conversation_runtime_keeps_receiver_after_text_delta_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };

        sender
            .send(ConversationWorkerEvent::progress(
                ConversationEvent::AssistantDelta {
                    content: "partial".to_string(),
                },
            ))
            .expect("assistant delta event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::AssistantDelta {
                content: "partial".to_string(),
            })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn conversation_runtime_buffers_session_events_without_ui_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            provider_turn_started: false,
            session_messages: Vec::new(),
        };
        let message = Message::text(MessageRole::Assistant, "stored");

        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderTurnStarted,
            ))
            .expect("provider event should be queued");
        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderContextMessage {
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
    fn conversation_interrupt_keeps_receiver_until_worker_terminal_event() {
        let (_sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
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
            super::TurnAttemptOutcome::TimedOut(value) => assert_eq!(value, "repaired"),
            super::TurnAttemptOutcome::Completed(_) => {
                panic!("timeout should mark the attempt as timed out")
            }
            super::TurnAttemptOutcome::TimedOutAfterGrace
            | super::TurnAttemptOutcome::CancelledAfterGrace => {
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
            super::TurnAttemptOutcome::TimedOutAfterGrace
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
            super::TurnAttemptOutcome::CancelledAfterGrace
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
    async fn conversation_worker_reports_interrupted_when_pre_cancelled() {
        let request = PreparedConversationRequest::new(
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

        super::run_conversation_worker(
            request,
            executor,
            RuntimeRequestPolicy::default(),
            cancellation,
            ConversationPermissionBroker::default(),
            sender,
        )
        .await;

        assert_eq!(
            receiver.recv().expect("worker should emit an event"),
            ConversationWorkerEvent::progress(ConversationEvent::Interrupted)
        );
    }
}
