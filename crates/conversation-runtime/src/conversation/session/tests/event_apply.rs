use super::support::*;

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
        upstream_context_tokens: None,
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
        upstream_context_tokens: None,
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
        cancellation: Some(CancellationToken::new()),
        target: Some(RuntimeTarget::provider("provider", "model")),
        permission_broker: None,
        pending_session_id: None,
        pending_user_entry_id: None,
        session_items: Vec::new(),
        upstream_context_tokens: None,
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
        upstream_context_tokens: None,
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
        upstream_context_tokens: None,
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
        upstream_context_tokens: None,
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
        upstream_context_tokens: None,
    };

    assert!(runtime.interrupt());
    assert!(runtime.is_running());
    assert!(runtime.current_target().is_some());
}
