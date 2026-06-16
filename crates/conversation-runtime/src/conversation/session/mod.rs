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

mod context_repair;
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
    pending_session_id: Option<SessionId>,
    pending_user_entry_id: Option<String>,
    session_items: Vec<PersistedConversationItem>,
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
        self.pending_session_id = None;
        self.pending_user_entry_id = None;
        self.session_items.clear();
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

    pub fn try_recv_event(&mut self) -> Option<ConversationEvent> {
        loop {
            let event = match self.receiver.as_ref()?.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.clear_runtime_state();
                    return Some(ConversationEvent::Failed {
                        message: "conversation request stopped before completion".to_string(),
                    });
                }
            };

            match event {
                ConversationWorkerEvent::Progress(event) => {
                    if event.is_terminal() {
                        self.clear_runtime_state();
                    }
                    return Some(event);
                }
                ConversationWorkerEvent::Session(event) => {
                    self.apply_session_event(event);
                }
                ConversationWorkerEvent::Finished { response, metrics } => {
                    self.clear_runtime_state();
                    return Some(ConversationEvent::Finished { response, metrics });
                }
            }
        }
    }

    fn apply_session_event(&mut self, event: ConversationDelta) {
        match event {
            ConversationDelta::ProviderTurnStarted {
                session_id,
                user_entry_id: Some(user_entry_id),
            } => {
                if let Some(session_id) = session_id {
                    self.pending_session_id = Some(session_id);
                }
                self.pending_user_entry_id = Some(user_entry_id);
            }
            ConversationDelta::ProviderTurnStarted { session_id, .. } => {
                if let Some(session_id) = session_id {
                    self.pending_session_id = Some(session_id);
                }
                // retry 重放可能只表示“turn 已经持久化过”，不能清掉首次 entry id。
            }
            ConversationDelta::ProviderContextItem { entry_id, item } => {
                self.session_items
                    .push(PersistedConversationItem { entry_id, item });
            }
        }
    }

    fn clear_runtime_state(&mut self) {
        self.receiver = None;
        self.cancellation = None;
        self.target = None;
        self.pending_session_id = None;
        self.pending_user_entry_id = None;
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
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
    let provider_context_items_started = Arc::new(AtomicBool::new(false));
    let provider_context_repair_ledger =
        Arc::new(Mutex::new(ProviderContextRepairLedger::default()));
    let (session_sender, session_receiver) = tokio_mpsc::unbounded_channel();
    let session_actor_cancellation = cancellation.clone();
    let session_actor = tokio::spawn(run_session_persistence_actor(
        request.persistence_cloned(),
        session_receiver,
        sender.clone(),
        session_actor_cancellation,
    ));
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let progress_session_sender = session_sender.clone();
        let attempt_provider_context_items_started = Arc::clone(&provider_context_items_started);
        let attempt_provider_context_repair_ledger = Arc::clone(&provider_context_repair_ledger);
        let attempt_cancellation = cancellation.child_token();
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
                    crate::conversation::ConversationProgress::ProviderTurnStarted => {
                        let _ = progress_session_sender
                            .send(SessionPersistenceCommand::ProviderTurnStarted);
                    }
                    crate::conversation::ConversationProgress::ProviderContextItem { item } => {
                        attempt_provider_context_items_started.store(true, Ordering::Relaxed);
                        attempt_provider_context_repair_ledger
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .observe(&item);
                        let _ = progress_session_sender
                            .send(SessionPersistenceCommand::ProviderContextItem(item));
                    }
                    crate::conversation::ConversationProgress::ToolActivityStarted { activity } => {
                        let _ = progress_session_sender.send(
                            SessionPersistenceCommand::ToolActivityStarted(activity.clone()),
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
                        let _ = progress_session_sender.send(
                            SessionPersistenceCommand::ToolActivityUpdated(update.clone()),
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
                        let _ = progress_session_sender.send(
                            SessionPersistenceCommand::TerminalSnapshot(snapshot.clone()),
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
    session_sender: &tokio_mpsc::UnboundedSender<SessionPersistenceCommand>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
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
    sender: &tokio_mpsc::UnboundedSender<SessionPersistenceCommand>,
    content: &'static str,
) {
    for item in take_provider_context_repair_items(ledger, content) {
        let _ = sender.send(SessionPersistenceCommand::ProviderContextItem(item));
    }
}

async fn retry_conversation_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    session_sender: &tokio_mpsc::UnboundedSender<SessionPersistenceCommand>,
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
    use runtime_domain::{
        request_policy::RuntimeRequestPolicy,
        session::{
            RuntimeTarget, RuntimeTerminalSnapshot, RuntimeToolActivity,
            RuntimeToolActivityContent, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
            RuntimeToolActivityUpdate, RuntimeToolKind, TranscriptReplayItem,
        },
    };
    use session_store::{
        LocalSessionStore, SessionHeader, SessionId, SessionStore, SessionStoreError,
    };
    use tokio::sync::mpsc as tokio_mpsc;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::ToolExecutorRegistry;

    use super::persistence::SessionPersistenceError;
    use super::{
        ConversationDelta, ConversationEvent, ConversationPermissionBroker, ConversationWorker,
        ConversationWorkerEvent,
    };
    use crate::{
        ConversationResponse, PreparedConversationRequest, ProviderConversation, ProviderKind,
        conversation::PersistedConversationItem,
    };

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
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
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
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
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
                response: ConversationResponse::assistant_text("完成"),
                metrics: None,
            })
            .expect("finish event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(ConversationEvent::Finished {
                response: ConversationResponse::assistant_text("完成"),
                metrics: None,
            })
        );
        assert!(!runtime.is_running());
        assert!(runtime.take_session_items().is_empty());
    }

    #[test]
    fn conversation_runtime_keeps_receiver_after_token_estimate_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
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
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
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
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
        };
        let message = ConversationItem::text(Role::Assistant, "stored");

        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderTurnStarted {
                    session_id: None,
                    user_entry_id: Some("user-1".to_string()),
                },
            ))
            .expect("provider event should be queued");
        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderContextItem {
                    entry_id: Some("assistant-1".to_string()),
                    item: message.clone(),
                },
            ))
            .expect("message event should be queued");

        assert_eq!(runtime.try_recv_event(), None);
        assert_eq!(
            runtime.take_pending_user_entry_id().as_deref(),
            Some("user-1")
        );
        assert_eq!(
            runtime.take_session_items(),
            vec![PersistedConversationItem {
                entry_id: Some("assistant-1".to_string()),
                item: message,
            }]
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn conversation_runtime_preserves_turn_entry_id_when_retry_replays_turn_start() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
        };

        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderTurnStarted {
                    session_id: None,
                    user_entry_id: Some("user-1".to_string()),
                },
            ))
            .expect("first provider turn start should queue");
        sender
            .send(ConversationWorkerEvent::Session(
                ConversationDelta::ProviderTurnStarted {
                    session_id: None,
                    user_entry_id: None,
                },
            ))
            .expect("retry provider turn start should queue");

        assert_eq!(runtime.try_recv_event(), None);
        assert_eq!(
            runtime.take_pending_user_entry_id().as_deref(),
            Some("user-1")
        );
    }

    #[test]
    fn conversation_interrupt_keeps_receiver_until_worker_terminal_event() {
        let (_sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("provider", "model")),
            permission_broker: None,
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
        };

        assert!(runtime.interrupt());
        assert!(runtime.is_running());
        assert!(runtime.current_target().is_some());
    }

    #[test]
    fn conversation_worker_persists_config_change_and_flushes_finished_turn() {
        let root = tempdir_path("worker-persistence");
        let work_dir = root.join("workspace");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let store =
            Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
        let store_trait: Arc<dyn SessionStore> = store.clone();
        let mut conversation = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
        )
        .expect("persisted conversation should initialize");
        let user = ConversationItem::text(Role::User, "hello");
        let request = conversation
            .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                user.clone(),
            ))
            .expect("turn should prepare");
        let assistant = ConversationItem::text(Role::Assistant, "hi");
        let (sender, receiver) = mpsc::channel();
        let mut runtime = ConversationWorker {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::provider("local", "qwen3")),
            permission_broker: None,
            pending_session_id: None,
            pending_user_entry_id: None,
            session_items: Vec::new(),
        };
        let sender_copy = sender.clone();
        let persistence = request.persistence_cloned();
        let cancellation = CancellationToken::new();
        let mut state = super::SessionPersistenceState::default();
        run_persistence(super::persist_turn_start(
            persistence.as_ref(),
            &sender_copy,
            &cancellation,
            &mut state,
        ))
        .expect("turn start should persist config and user");
        run_persistence(super::persist_context_item(
            persistence.as_ref(),
            &sender_copy,
            &cancellation,
            assistant.clone(),
            &mut state,
        ))
        .expect("assistant item should persist");
        sender
            .send(ConversationWorkerEvent::Finished {
                response: ConversationResponse::assistant_text("hi"),
                metrics: None,
            })
            .expect("finish event should queue");

        assert!(matches!(
            runtime.try_recv_event(),
            Some(ConversationEvent::Finished { .. })
        ));

        let metas = run_store(store.list_sessions(work_dir.to_string_lossy().as_ref()))
            .expect("session meta should list");
        assert_eq!(metas.len(), 1);
        let resolved = run_store(store.resolve(&metas[0].session_id, None))
            .expect("resolved items should be readable");
        let jsonl = fs::read_to_string(&metas[0].jsonl_path).expect("jsonl should be readable");

        assert_eq!(resolved, vec![user, assistant]);
        assert!(jsonl.contains("\"type\":\"config_change\""));
    }

    #[test]
    fn flush_session_persistence_preserves_store_error_source() {
        let root = tempdir_path("worker-flush-error-source");
        let work_dir = root.join("workspace");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let store =
            Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
        let missing_session_id = SessionId::new();
        let store_trait: Arc<dyn SessionStore> = store;
        let mut conversation = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
        )
        .expect("persisted conversation should initialize");
        conversation.set_session_id(missing_session_id.clone());
        let request = conversation
            .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let error = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(async {
                let (command_sender, command_receiver) = tokio_mpsc::unbounded_channel();
                let (event_sender, _event_receiver) = mpsc::channel();
                let actor = tokio::spawn(super::run_session_persistence_actor(
                    request.persistence_cloned(),
                    command_receiver,
                    event_sender,
                    CancellationToken::new(),
                ));
                let error = super::flush_session_persistence(&command_sender)
                    .await
                    .expect_err("flush failure should preserve typed source");
                drop(command_sender);
                actor.await.expect("persistence actor should stop cleanly");
                error
            });

        assert!(matches!(
            error.as_ref(),
            SessionPersistenceError::Flush {
                source: SessionStoreError::SessionNotFound { session_id }
            } if session_id == &missing_session_id
        ));
    }

    #[test]
    fn persistence_helpers_store_rich_tool_replay_without_duplicate_tool_result() {
        let root = tempdir_path("worker-tool-replay-persistence");
        let work_dir = root.join("workspace");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let store =
            Arc::new(run_store(LocalSessionStore::open_in(root)).expect("local store should open"));
        let store_trait: Arc<dyn SessionStore> = store.clone();
        let mut conversation = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
        )
        .expect("persisted conversation should initialize");
        let request = conversation
            .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "edit file"),
            ))
            .expect("turn should prepare");
        let (sender, _receiver) = mpsc::channel();
        let persistence = request.persistence_cloned();
        let cancellation = CancellationToken::new();
        let mut state = super::SessionPersistenceState::default();
        let started_activity = RuntimeToolActivity {
            activity_id: "call-1".to_string(),
            title: "Write src/lib.rs".to_string(),
            kind: RuntimeToolKind::Write,
            status: RuntimeToolActivityStatus::InProgress,
            content: vec![RuntimeToolActivityContent::Text("src/lib.rs".to_string())],
            locations: Vec::new(),
            raw_input: Some(RuntimeToolActivityRawValue::from(
                r#"{"path":"src/lib.rs"}"#,
            )),
            raw_output: None,
        };
        let final_update = RuntimeToolActivityUpdate {
            activity_id: "call-1".to_string(),
            title: Some("Write src/lib.rs".to_string()),
            kind: Some(RuntimeToolKind::Write),
            status: Some(RuntimeToolActivityStatus::Completed),
            content: Some(vec![RuntimeToolActivityContent::Diff {
                path: "src/lib.rs".to_string(),
                old_text: Some("old".to_string()),
                new_text: "new".to_string(),
                is_truncated: false,
            }]),
            locations: Some(Vec::new()),
            raw_input: Some(RuntimeToolActivityRawValue::from(
                r#"{"path":"src/lib.rs"}"#,
            )),
            raw_output: Some(RuntimeToolActivityRawValue::tool_result(
                "plain provider output",
                None,
            )),
        };
        let terminal_snapshot = RuntimeTerminalSnapshot {
            terminal_id: "call-1".to_string(),
            command: Some("write src/lib.rs".to_string()),
            cwd: Some(work_dir.display().to_string()),
            output: "terminal output".to_string(),
            truncated: false,
            exit_status: None,
            released: true,
        };

        run_persistence(super::persist_turn_start(
            persistence.as_ref(),
            &sender,
            &cancellation,
            &mut state,
        ))
        .expect("turn start should persist");
        run_persistence(super::persist_tool_activity_started(
            persistence.as_ref(),
            started_activity,
            &mut state,
        ))
        .expect("started activity should persist");
        run_persistence(super::persist_tool_activity_update(
            persistence.as_ref(),
            final_update,
            &mut state,
        ))
        .expect("final activity should persist");
        run_persistence(super::persist_terminal_snapshot(
            persistence.as_ref(),
            terminal_snapshot.clone(),
            &state,
        ))
        .expect("terminal snapshot should persist");
        run_persistence(super::persist_context_item(
            persistence.as_ref(),
            &sender,
            &cancellation,
            ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("plain provider output".to_string())],
                false,
            ),
            &mut state,
        ))
        .expect("tool result item should persist");

        let meta = run_store(store.list_sessions(work_dir.to_string_lossy().as_ref()))
            .expect("session meta should list")
            .into_iter()
            .next()
            .expect("session should exist");
        let restored =
            run_store(store.load_session(&meta.session_id, None)).expect("session should load");

        assert_eq!(restored.transcript.len(), 3);
        assert!(matches!(
            &restored.transcript[0],
            TranscriptReplayItem::Message {
                role: runtime_domain::session::TranscriptReplayRole::User,
                content,
            } if content == "edit file"
        ));
        assert!(matches!(
            &restored.transcript[1],
            TranscriptReplayItem::ToolActivity { activity }
                if activity.activity_id == "call-1"
                    && matches!(
                        activity.content.as_slice(),
                        [RuntimeToolActivityContent::Diff { path, old_text, new_text, is_truncated }]
                            if path == "src/lib.rs"
                                && old_text.as_deref() == Some("old")
                                && new_text == "new"
                                && !is_truncated
                    )
        ));
        assert_eq!(
            restored.transcript[2],
            TranscriptReplayItem::TerminalSnapshot {
                snapshot: terminal_snapshot
            }
        );
    }

    #[tokio::test]
    async fn soft_timeout_pauses_while_permission_is_waiting() {
        let parent_cancellation = CancellationToken::new();
        let attempt_cancellation = parent_cancellation.child_token();
        let timeout_pause = super::ConversationTimeoutPause::default();
        let future_timeout_pause = timeout_pause.clone();

        let outcome = super::run_with_soft_timeout(
            &parent_cancellation,
            &attempt_cancellation,
            Duration::from_millis(10),
            Duration::from_millis(5),
            timeout_pause,
            async move {
                let permission_wait = future_timeout_pause.pause();
                tokio::time::sleep(Duration::from_millis(30)).await;
                drop(permission_wait);
                "approved"
            },
        )
        .await;

        assert!(matches!(
            outcome,
            super::TurnAttemptOutcome::Completed("approved")
        ));
        assert!(
            !attempt_cancellation.is_cancelled(),
            "permission waits should not consume the request timeout budget"
        );
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
            super::ConversationTimeoutPause::default(),
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
            super::ConversationTimeoutPause::default(),
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
            super::ConversationTimeoutPause::default(),
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
    fn provider_context_repair_items_fill_unresolved_tool_calls() {
        let mut ledger = super::ProviderContextRepairLedger::default();

        ledger.observe(&ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![
                ToolCall::new("call-1", "read", "{}"),
                ToolCall::new("call-2", "search", "{}"),
            ],
        ));
        ledger.observe(&ConversationItem::tool_result(
            "call-1",
            vec![ContentBlock::Text("done".into())],
            false,
        ));

        let repair_items = ledger.take_repair_items(super::TOOL_EXECUTION_TIMED_OUT);

        assert_eq!(repair_items.len(), 1);
        let repair_item = &repair_items[0];
        match repair_item {
            ConversationItem::ToolResult {
                call_id,
                is_error,
                content,
            } => {
                assert_eq!(call_id, "call-2");
                assert!(*is_error);
                assert_eq!(content[0].as_text(), Some(super::TOOL_EXECUTION_TIMED_OUT));
            }
            _ => panic!("expected ToolResult item"),
        }
        assert!(
            ledger
                .take_repair_items(super::TOOL_EXECUTION_TIMED_OUT)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn conversation_worker_reports_interrupted_when_pre_cancelled() {
        let turn = runtime_domain::session::ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        );
        let request = PreparedConversationRequest::from_turn(
            &turn,
            vec![ConversationItem::text(Role::User, "hello")],
            None,
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

    fn run_store<T>(
        future: impl std::future::Future<Output = Result<T, SessionStoreError>>,
    ) -> Result<T, SessionStoreError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        runtime.block_on(future)
    }

    fn run_persistence<T>(
        future: impl std::future::Future<Output = Result<T, SessionPersistenceError>>,
    ) -> Result<T, SessionPersistenceError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        runtime.block_on(future)
    }

    fn sample_header(work_dir: &Path, model: &str) -> SessionHeader {
        SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.to_path_buf(),
            session_name: Some("worker-test".to_string()),
            initial_model: model.to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.7".to_string()),
        }
    }

    fn tempdir_path(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "lumos-conversation-worker-{label}-{}-{stamp}",
            std::process::id()
        ))
    }
}
