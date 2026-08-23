use super::support::*;
use runtime_domain::session::RuntimePermissionOptionKind;
use tool_runtime::{
    ToolCall as RuntimeToolCall, ToolDefinition, ToolKind, ToolPermissionDecision,
    ToolPermissionHandler, ToolPermissionPolicy, ToolPermissionPreview, ToolPermissionRequest,
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
            upstream_context_tokens: Some(48),
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
    assert_eq!(runtime.take_upstream_context_tokens(), Some(48));
}

#[test]
fn conversation_runtime_keeps_receiver_after_token_estimate_event() {
    let (sender, receiver) = mpsc::channel();
    let mut runtime = ConversationWorker {
        receiver: Some(receiver),
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
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
        worker_thread: None,
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: ConversationPermissionBroker::default(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
    };

    assert!(runtime.interrupt());
    assert!(runtime.is_running());
    assert!(runtime.current_target().is_some());
}

#[tokio::test]
async fn conversation_worker_reset_keeps_existing_permission_rules_for_the_next_turn() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = broker.handler(sender.clone());
    let cancellation = CancellationToken::new();
    let first = tokio::spawn(async move {
        handler
            .request_permission(permission_request_for_reset_test(), &cancellation)
            .await
    });

    let (request_id, allow_always_id) = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match receiver.try_recv() {
                Ok(ConversationEvent::PermissionRequested { request }) => {
                    let allow_always_id = request
                        .options
                        .into_iter()
                        .find(|option| option.kind == RuntimePermissionOptionKind::AllowAlways)
                        .map(|option| option.option_id)
                        .unwrap_or_else(|| {
                            panic!("permission request should expose an allow-always option")
                        });
                    break (request.request_id, allow_always_id);
                }
                Ok(other) => panic!("expected permission request event, got {other:?}"),
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("permission event sender disconnected")
                }
            }
        }
    })
    .await
    .expect("permission request should be emitted");

    broker
        .respond_permission(&request_id, Some(allow_always_id))
        .expect("first permission request should accept the session rule");
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Allow
    );

    let mut runtime = ConversationWorker::new(RuntimeEventNotifier::default());
    runtime.permission_broker = broker.clone();

    runtime
        .reset_after_clear()
        .expect("worker without a thread should reset cleanly");

    let second_handler = broker.handler(sender);
    let second_cancellation = CancellationToken::new();
    let second_decision = tokio::time::timeout(
        Duration::from_millis(100),
        second_handler
            .request_permission(permission_request_for_reset_test(), &second_cancellation),
    )
    .await
    .expect("matching rule should survive clear/new without another prompt");
    assert_eq!(second_decision, ToolPermissionDecision::Allow);
    assert!(
        matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "clear/new should not cause a matching rule to emit a new permission request"
    );
}

#[tokio::test]
async fn conversation_worker_context_change_cancels_turn_and_clears_permission_rules() {
    let broker = ConversationPermissionBroker::default();
    let (permission_sender, permission_receiver) = mpsc::channel();
    let handler = broker.handler(permission_sender.clone());
    let permission_cancellation = CancellationToken::new();
    let first = tokio::spawn(async move {
        handler
            .request_permission(
                permission_request_for_reset_test(),
                &permission_cancellation,
            )
            .await
    });
    let (request_id, allow_always_id) = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match permission_receiver.try_recv() {
                Ok(ConversationEvent::PermissionRequested { request }) => {
                    let allow_always_id = request
                        .options
                        .into_iter()
                        .find(|option| option.kind == RuntimePermissionOptionKind::AllowAlways)
                        .map(|option| option.option_id)
                        .expect("permission request should expose an allow-always option");
                    break (request.request_id, allow_always_id);
                }
                Ok(other) => panic!("expected permission request event, got {other:?}"),
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("permission event sender disconnected")
                }
            }
        }
    })
    .await
    .expect("permission request should be emitted");
    broker
        .respond_permission(&request_id, Some(allow_always_id))
        .expect("first permission request should accept the session rule");
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Allow
    );

    let (worker_sender, worker_receiver) = mpsc::channel();
    let turn_cancellation = CancellationToken::new();
    let mut runtime = ConversationWorker {
        receiver: Some(worker_receiver),
        worker_thread: None,
        cancellation: Some(turn_cancellation.clone()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: broker.clone(),
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
        event_notifier: RuntimeEventNotifier::default(),
    };

    runtime
        .reset_for_context_change()
        .expect("worker without a thread should reset cleanly");

    assert!(turn_cancellation.is_cancelled());
    assert!(!runtime.is_running());
    drop(worker_sender);

    let second_handler = broker.handler(permission_sender);
    let second_cancellation = CancellationToken::new();
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(permission_request_for_reset_test(), &second_cancellation)
            .await
    });
    let request_id = match tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match permission_receiver.try_recv() {
                Ok(event) => break event,
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("permission event sender disconnected")
                }
            }
        }
    })
    .await
    .expect("context change should force a new permission request")
    {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, None)
        .expect("new request should remain cancellable");
    assert!(matches!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
}

fn permission_request_for_reset_test() -> ToolPermissionRequest {
    ToolPermissionRequest::new(
        RuntimeToolCall::new(
            "reset-test-call",
            "write",
            serde_json::json!({
                "path": "TEMP.md",
                "content": "body",
            }),
        ),
        ToolDefinition::new("write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Ask),
    )
    .with_preview(ToolPermissionPreview {
        path: "TEMP.md".to_string(),
        old_text: None,
        new_text: "body".to_string(),
        is_truncated: false,
        snapshot: None,
    })
}
