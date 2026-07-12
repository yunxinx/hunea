//! 对话 session worker 的后台执行与进度汇聚。

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use tokio::sync::mpsc as tokio_mpsc;
use tokio_util::sync::CancellationToken;

use runtime_domain::{
    request_policy::RuntimeRequestPolicy,
    session::{ConversationEvent, RuntimeTarget},
};
use session_store::SessionId;
use tool_runtime::{SharedToolPermissionHandler, ToolExecutorRegistry};

use super::{
    ConversationPermissionBroker, ConversationTimeoutPause, PersistedConversationItem,
    TurnExecutionError, turn::run_prepared_conversation_with_progress,
};
use crate::PreparedConversationRequest;
use crate::{NotifyingSender, RuntimeEventNotifier};

mod context_repair;
mod event_apply;
mod persistence;
mod progress_mapping;
mod timeout;

use context_repair::{ProviderContextRepairLedger, take_provider_context_repair_items};
use persistence::{
    ConversationDelta, SessionPersistenceCommand, flush_session_persistence,
    run_session_persistence_actor,
};
use progress_mapping::{
    conversation_worker_event_from_progress, progress_sender_to_permission_sender,
};
use timeout::{TurnAttemptOutcome, run_with_soft_timeout};

#[cfg(test)]
use persistence::{
    SessionPersistenceState, persist_context_item, persist_terminal_snapshot,
    persist_tool_activity_started, persist_tool_activity_update, persist_turn_start,
};

const TIMEOUT_REPAIR_GRACE: Duration = Duration::from_secs(2);
const SESSION_PERSISTENCE_QUEUE_CAPACITY: usize = 256;
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";
const TOOL_EXECUTION_TIMED_OUT: &str = "Tool execution timed out";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConversationWorkerEvent {
    Progress(ConversationEvent),
    Session(ConversationDelta),
    Finished {
        response: runtime_domain::session::ConversationResponse,
        metrics: Option<crate::ProviderRequestMetrics>,
        upstream_context_tokens: Option<usize>,
    },
}

impl ConversationWorkerEvent {
    fn progress(event: ConversationEvent) -> Self {
        Self::Progress(event)
    }
}

type ConversationWorkerEventSender = NotifyingSender<ConversationWorkerEvent>;

/// `ConversationWorker` 管理对话请求的后台 worker 与取消状态。
pub struct ConversationWorker {
    receiver: Option<Receiver<ConversationWorkerEvent>>,
    pub cancellation: Option<CancellationToken>,
    pub target: Option<RuntimeTarget>,
    permission_broker: Option<ConversationPermissionBroker>,
    pending_session_id: Option<SessionId>,
    pending_user_entry_id: Option<String>,
    session_items: Vec<PersistedConversationItem>,
    upstream_context_tokens: Option<usize>,
    event_notifier: RuntimeEventNotifier,
}

impl ConversationWorker {
    pub fn new(event_notifier: RuntimeEventNotifier) -> Self {
        Self {
            receiver: None,
            cancellation: None,
            target: None,
            permission_broker: None,
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
            upstream_context_tokens: None,
            event_notifier,
        }
    }

    pub fn start(
        &mut self,
        request: PreparedConversationRequest,
        executor: ToolExecutorRegistry,
        request_policy: RuntimeRequestPolicy,
    ) {
        let (sender, receiver) = mpsc::channel();
        let sender = ConversationWorkerEventSender::new(sender, self.event_notifier.clone());
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        let target = request.target();
        let permission_broker = ConversationPermissionBroker::default();
        let thread_permission_broker = permission_broker.clone();
        thread::spawn(move || {
            let _exit_notification = sender.notify_on_drop();
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
        self.pending_session_id = None;
        self.pending_user_entry_id = None;
        self.session_items.clear();
        self.upstream_context_tokens = None;
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
        self.pending_session_id = None;
        self.pending_user_entry_id = None;
        self.session_items.clear();
        self.upstream_context_tokens = None;
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

    pub fn take_pending_user_entry_id(&mut self) -> Option<String> {
        self.pending_user_entry_id.take()
    }

    pub fn take_pending_session_id(&mut self) -> Option<SessionId> {
        self.pending_session_id.take()
    }

    pub fn take_session_items(&mut self) -> Vec<PersistedConversationItem> {
        std::mem::take(&mut self.session_items)
    }

    pub fn take_upstream_context_tokens(&mut self) -> Option<usize> {
        self.upstream_context_tokens.take()
    }
}

impl Default for ConversationWorker {
    fn default() -> Self {
        Self::new(RuntimeEventNotifier::default())
    }
}

async fn run_conversation_worker(
    request: PreparedConversationRequest,
    executor: ToolExecutorRegistry,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    permission_broker: ConversationPermissionBroker,
    sender: ConversationWorkerEventSender,
) {
    let provider_context_items_started = Arc::new(AtomicBool::new(false));
    let provider_context_repair_ledger =
        Arc::new(Mutex::new(ProviderContextRepairLedger::default()));
    let (session_sender, session_receiver) =
        tokio_mpsc::channel(SESSION_PERSISTENCE_QUEUE_CAPACITY);
    let session_actor_cancellation = cancellation.clone();
    let session_actor = tokio::spawn(run_session_persistence_actor(
        request.persistence_cloned(),
        session_receiver,
        sender.clone(),
        session_actor_cancellation,
    ));
    if cancellation.is_cancelled() {
        let _ = sender.send(ConversationWorkerEvent::progress(
            ConversationEvent::Interrupted,
        ));
        drop(session_sender);
        let _ = session_actor.await;
        return;
    }
    let _ = session_sender
        .send(SessionPersistenceCommand::ProviderTurnStarted)
        .await;
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let progress_session_sender = session_sender.clone();
        let attempt_provider_context_items_started = Arc::clone(&provider_context_items_started);
        let attempt_provider_context_repair_ledger = Arc::clone(&provider_context_repair_ledger);
        let attempt_cancellation = cancellation.child_token();
        let progress_attempt_cancellation = attempt_cancellation.clone();
        let timeout_pause = ConversationTimeoutPause::default();
        let permission_handler: SharedToolPermissionHandler =
            std::sync::Arc::new(permission_broker.handler(
                progress_sender_to_permission_sender(sender.clone()),
                timeout_pause.clone(),
            ));
        let attempt_result = run_with_soft_timeout(
            &cancellation,
            &attempt_cancellation,
            request_policy.timeout(),
            TIMEOUT_REPAIR_GRACE,
            timeout_pause,
            run_prepared_conversation_with_progress(
                &request,
                executor.clone(),
                &attempt_cancellation,
                request_policy.tool_max_turns(),
                Some(permission_handler),
                move |progress| match progress {
                    crate::conversation::ConversationProgress::ProviderTurnStarted => {}
                    crate::conversation::ConversationProgress::ProviderContextItem { item } => {
                        attempt_provider_context_items_started.store(true, Ordering::Relaxed);
                        attempt_provider_context_repair_ledger
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .observe(&item);
                        try_send_session_persistence(
                            &progress_session_sender,
                            SessionPersistenceCommand::ProviderContextItem(item),
                            &progress_attempt_cancellation,
                            &progress_sender,
                        );
                    }
                    crate::conversation::ConversationProgress::ToolActivityStarted { activity } => {
                        try_send_session_persistence(
                            &progress_session_sender,
                            SessionPersistenceCommand::ToolActivityStarted(activity.clone()),
                            &progress_attempt_cancellation,
                            &progress_sender,
                        );
                        if let Some(event) = conversation_worker_event_from_progress(
                            crate::conversation::ConversationProgress::ToolActivityStarted {
                                activity,
                            },
                        ) {
                            let _ = progress_sender.send(event);
                        }
                    }
                    crate::conversation::ConversationProgress::ToolActivityUpdated { update } => {
                        try_send_session_persistence(
                            &progress_session_sender,
                            SessionPersistenceCommand::ToolActivityUpdated(update.clone()),
                            &progress_attempt_cancellation,
                            &progress_sender,
                        );
                        if let Some(event) = conversation_worker_event_from_progress(
                            crate::conversation::ConversationProgress::ToolActivityUpdated {
                                update,
                            },
                        ) {
                            let _ = progress_sender.send(event);
                        }
                    }
                    crate::conversation::ConversationProgress::TerminalUpdated { snapshot } => {
                        try_send_session_persistence(
                            &progress_session_sender,
                            SessionPersistenceCommand::TerminalSnapshot(snapshot.clone()),
                            &progress_attempt_cancellation,
                            &progress_sender,
                        );
                        if let Some(event) = conversation_worker_event_from_progress(
                            crate::conversation::ConversationProgress::TerminalUpdated { snapshot },
                        ) {
                            let _ = progress_sender.send(event);
                        }
                    }
                    other => {
                        if let Some(event) = conversation_worker_event_from_progress(other) {
                            let _ = progress_sender.send(event);
                        }
                    }
                },
            ),
        )
        .await;
        let can_retry_from_original_request =
            !provider_context_items_started.load(Ordering::Relaxed);

        match attempt_result {
            TurnAttemptOutcome::TimedOut(Err(_)) | TurnAttemptOutcome::TimedOutAfterGrace
                if attempt < request_policy.attempts() && can_retry_from_original_request =>
            {
                permission_broker.cancel_all();
                if retry_conversation_after_attempt(
                    attempt,
                    &request_policy,
                    &cancellation,
                    &session_sender,
                    &sender,
                )
                .await
                {
                    drop(session_sender);
                    let _ = session_actor.await;
                    return;
                }
            }
            TurnAttemptOutcome::TimedOut(Err(_)) | TurnAttemptOutcome::TimedOutAfterGrace => {
                permission_broker.cancel_all();
                send_repair_items(
                    provider_context_repair_ledger.as_ref(),
                    &session_sender,
                    TOOL_EXECUTION_TIMED_OUT,
                    &cancellation,
                    &sender,
                );
                send_terminal_after_session_persistence(
                    &session_sender,
                    &sender,
                    ConversationEvent::Failed {
                        message: format!(
                            "Conversation request timed out after {}s",
                            request_policy.timeout().as_secs()
                        ),
                    },
                )
                .await;
                drop(session_sender);
                let _ = session_actor.await;
                return;
            }
            TurnAttemptOutcome::Completed(Ok(completion))
            | TurnAttemptOutcome::TimedOut(Ok(completion)) => {
                permission_broker.cancel_all();
                if let Err(error) = flush_session_persistence(&session_sender).await {
                    let _ = sender.send(ConversationWorkerEvent::progress(
                        ConversationEvent::Failed {
                            message: error.to_string(),
                        },
                    ));
                    drop(session_sender);
                    let _ = session_actor.await;
                    return;
                }
                let _ = sender.send(ConversationWorkerEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                    upstream_context_tokens: completion.upstream_context_tokens,
                });
                drop(session_sender);
                let _ = session_actor.await;
                return;
            }
            TurnAttemptOutcome::Completed(Err(TurnExecutionError::Cancelled)) => {
                permission_broker.cancel_all();
                send_repair_items(
                    provider_context_repair_ledger.as_ref(),
                    &session_sender,
                    TOOL_EXECUTION_INTERRUPTED,
                    &cancellation,
                    &sender,
                );
                send_terminal_after_session_persistence(
                    &session_sender,
                    &sender,
                    ConversationEvent::Interrupted,
                )
                .await;
                drop(session_sender);
                let _ = session_actor.await;
                return;
            }
            TurnAttemptOutcome::CancelledAfterGrace => {
                permission_broker.cancel_all();
                send_repair_items(
                    provider_context_repair_ledger.as_ref(),
                    &session_sender,
                    TOOL_EXECUTION_INTERRUPTED,
                    &cancellation,
                    &sender,
                );
                send_terminal_after_session_persistence(
                    &session_sender,
                    &sender,
                    ConversationEvent::Interrupted,
                )
                .await;
                drop(session_sender);
                let _ = session_actor.await;
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
                    &session_sender,
                    &sender,
                )
                .await
                {
                    drop(session_sender);
                    let _ = session_actor.await;
                    return;
                }
            }
            TurnAttemptOutcome::Completed(Err(error)) => {
                permission_broker.cancel_all();
                send_repair_items(
                    provider_context_repair_ledger.as_ref(),
                    &session_sender,
                    TOOL_EXECUTION_INTERRUPTED,
                    &cancellation,
                    &sender,
                );
                send_terminal_after_session_persistence(
                    &session_sender,
                    &sender,
                    ConversationEvent::Failed {
                        message: error.to_string(),
                    },
                )
                .await;
                drop(session_sender);
                let _ = session_actor.await;
                return;
            }
        }
    }

    drop(session_sender);
    let _ = session_actor.await;
}

async fn send_terminal_after_session_persistence(
    session_sender: &tokio_mpsc::Sender<SessionPersistenceCommand>,
    sender: &ConversationWorkerEventSender,
    terminal_event: ConversationEvent,
) {
    let event = match flush_session_persistence(session_sender).await {
        Ok(()) => terminal_event,
        Err(error) => ConversationEvent::Failed {
            message: error.to_string(),
        },
    };
    let _ = sender.send(ConversationWorkerEvent::progress(event));
}

fn send_repair_items(
    ledger: &Mutex<ProviderContextRepairLedger>,
    sender: &tokio_mpsc::Sender<SessionPersistenceCommand>,
    content: &'static str,
    conversation_cancellation: &CancellationToken,
    progress_sender: &ConversationWorkerEventSender,
) {
    for item in take_provider_context_repair_items(ledger, content) {
        if !try_send_session_persistence(
            sender,
            SessionPersistenceCommand::ProviderContextItem(item),
            conversation_cancellation,
            progress_sender,
        ) {
            break;
        }
    }
}

async fn retry_conversation_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    session_sender: &tokio_mpsc::Sender<SessionPersistenceCommand>,
    sender: &ConversationWorkerEventSender,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(ConversationWorkerEvent::progress(
        ConversationEvent::Retrying {
            message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
        },
    ));
    tokio::select! {
        _ = cancellation.cancelled() => {
            send_terminal_after_session_persistence(
                session_sender,
                sender,
                ConversationEvent::Interrupted,
            ).await;
            true
        }
        _ = tokio::time::sleep(request_policy.delay_for_retry(retry)) => false,
    }
}

fn try_send_session_persistence(
    sender: &tokio_mpsc::Sender<SessionPersistenceCommand>,
    command: SessionPersistenceCommand,
    conversation_cancellation: &CancellationToken,
    progress_sender: &ConversationWorkerEventSender,
) -> bool {
    match sender.try_send(command) {
        Ok(()) => true,
        Err(tokio_mpsc::error::TrySendError::Full(_)) => {
            conversation_cancellation.cancel();
            let _ = progress_sender.send(ConversationWorkerEvent::progress(
                ConversationEvent::Failed {
                    message: "conversation session persistence queue is full".to_string(),
                },
            ));
            false
        }
        Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
            conversation_cancellation.cancel();
            let _ = progress_sender.send(ConversationWorkerEvent::progress(
                ConversationEvent::Failed {
                    message: "conversation session persistence worker stopped".to_string(),
                },
            ));
            false
        }
    }
}

#[cfg(test)]
mod tests;
