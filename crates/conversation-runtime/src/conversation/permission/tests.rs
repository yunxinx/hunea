use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

use runtime_domain::session::RuntimeToolActivityContent;
use tool_runtime::{
    ToolCall, ToolDefinition, ToolKind, ToolPermissionPolicy, ToolPermissionPreview,
};

use super::*;

const EXPECTED_ALLOW_ALWAYS_OPTION_ID: &str = "allow_always";
const EXPECTED_REJECT_ALWAYS_OPTION_ID: &str = "reject_always";

fn permission_request() -> ToolPermissionRequest {
    ToolPermissionRequest::new(
        ToolCall::new(
            "write",
            "write",
            serde_json::json!({
                "path": "TEMP.md",
                "content": "body",
            }),
        ),
        ToolDefinition::new("write")
            .with_label("Write")
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

fn permission_request_with_preview() -> ToolPermissionRequest {
    permission_request().with_preview(ToolPermissionPreview {
        path: "TEMP.md".to_string(),
        old_text: Some("old\n".to_string()),
        new_text: "new\n".to_string(),
        is_truncated: false,
        snapshot: None,
    })
}

async fn recv_event(receiver: &mpsc::Receiver<ConversationEvent>) -> ConversationEvent {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match receiver.try_recv() {
                Ok(event) => return event,
                Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                Err(mpsc::TryRecvError::Disconnected) => {
                    panic!("permission event sender disconnected")
                }
            }
        }
    })
    .await
    .expect("permission event should be emitted")
}

#[tokio::test]
async fn conversation_permission_handler_round_trips_allow_response() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();

    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });

    let event = recv_event(&receiver).await;
    let request_id = match event {
        ConversationEvent::PermissionRequested { request } => {
            assert_eq!(request.title, Some("Write TEMP.md".to_string()));
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::AllowOnce),
                Some(ALLOW_ONCE_OPTION_ID.to_string())
            );
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::RejectOnce),
                Some(REJECT_ONCE_OPTION_ID.to_string())
            );
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::AllowAlways),
                Some(EXPECTED_ALLOW_ALWAYS_OPTION_ID.to_string())
            );
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::RejectAlways),
                Some(EXPECTED_REJECT_ALWAYS_OPTION_ID.to_string())
            );
            assert!(
                request.tool_activity.is_some(),
                "conversation permission requests should include a tool activity preview"
            );
            assert_eq!(
                request
                    .tool_activity
                    .as_ref()
                    .map(|activity| activity.activity_id.as_str()),
                Some("write"),
                "tool activity preview should keep the original provider tool call id"
            );
            request.request_id
        }
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept allow response");

    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[test]
fn conversation_permission_response_recovers_from_poisoned_pending_lock() {
    let broker = ConversationPermissionBroker::default();
    let request_id = broker.next_request_id();
    let (response_sender, mut response_receiver) = oneshot::channel();
    assert!(broker.register(
        broker.context_generation(),
        request_id.clone(),
        response_sender,
    ));

    let poison_broker = broker.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poison_broker
            .pending
            .lock()
            .expect("test should acquire the pending lock before poisoning");
        panic!("poison pending lock");
    })
    .join();

    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("poisoned lock should not prevent responding to pending permission");
    assert_eq!(
        response_receiver
            .try_recv()
            .expect("permission response should be delivered"),
        Some(ALLOW_ONCE_OPTION_ID.to_string())
    );
}

#[test]
fn permission_rules_recover_from_a_poisoned_lock() {
    let broker = ConversationPermissionBroker::default();
    let generation = broker.context_generation();
    let rule =
        ToolPermissionRule::from_request(&permission_request(), ToolPermissionRuleBehavior::Allow)
            .expect("valid permission request should create a rule");

    let poison_broker = broker.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poison_broker
            .rules
            .lock()
            .expect("test should acquire the rules lock before poisoning");
        panic!("poison rules lock");
    })
    .join();

    broker.insert_rule_if_active(generation, &CancellationToken::new(), rule);
    assert_eq!(
        broker.evaluate(&permission_request()),
        Some(ToolPermissionRuleBehavior::Allow)
    );

    broker.clear_permission_context();
    assert_eq!(broker.evaluate(&permission_request()), None);
}

#[tokio::test]
async fn conversation_permission_request_preserves_tool_diff_preview() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();

    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request_with_preview(), &task_cancellation)
            .await
    });

    let event = recv_event(&receiver).await;
    let request_id = match event {
        ConversationEvent::PermissionRequested { request } => {
            assert!(matches!(
                request
                    .tool_activity
                    .as_ref()
                    .and_then(|activity| activity.content.as_ref())
                    .and_then(|content| content.first()),
                Some(RuntimeToolActivityContent::Diff {
                    path,
                    old_text,
                    new_text,
                    ..
                }) if path == "TEMP.md"
                    && old_text.as_deref() == Some("old\n")
                    && new_text == "new\n"
            ));
            request.request_id
        }
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept allow response");

    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn conversation_permission_handler_denies_reject_response_without_leaking_pending_request() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();

    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });

    let event = recv_event(&receiver).await;
    let request_id = match event {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(REJECT_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept reject response");

    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Deny {
            message: "Tool permission denied: write user rejected the tool call".to_string()
        }
    );
    assert!(
        broker.respond_permission(&request_id, None).is_err(),
        "completed conversation permission requests should be removed"
    );
    assert!(
        broker.rules_guard().is_empty(),
        "reject-once must not persist a session rule"
    );
}

#[tokio::test]
async fn unknown_permission_option_denies_without_persisting_a_rule() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();
    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some("unknown-option".to_string()))
        .expect("pending request should accept the runtime response envelope");

    assert!(matches!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(broker.rules_guard().is_empty());
}

#[tokio::test]
async fn empty_permission_response_denies_without_persisting_a_rule() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();
    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, None)
        .expect("pending request should accept cancellation response");

    assert!(matches!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(broker.rules_guard().is_empty());
}

#[tokio::test]
async fn disconnected_permission_event_receiver_denies_without_persisting_a_rule() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    drop(receiver);
    let handler = broker.handler(sender);
    let cancellation = CancellationToken::new();

    let decision = handler
        .request_permission(permission_request(), &cancellation)
        .await;

    assert!(matches!(decision, ToolPermissionDecision::Deny { .. }));
    assert!(broker.rules_guard().is_empty());
    assert!(broker.pending_guard().is_empty());
}

#[tokio::test]
async fn conversation_permission_cancel_all_denies_pending_request() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();

    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });

    let event = recv_event(&receiver).await;
    let request_id = match event {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker.cancel_all();

    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Deny {
            message: "Tool permission denied: write user rejected the tool call".to_string()
        }
    );
    assert!(
        broker.respond_permission(&request_id, None).is_err(),
        "cancelled conversation permission requests should be removed"
    );
}

#[tokio::test]
async fn safe_file_permission_request_exposes_all_runtime_approval_options() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();
    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();

    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });

    let event = recv_event(&receiver).await;
    let request_id = match event {
        ConversationEvent::PermissionRequested { request } => {
            assert_eq!(request.options.len(), 4);
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::AllowAlways),
                Some(EXPECTED_ALLOW_ALWAYS_OPTION_ID.to_string())
            );
            assert_eq!(
                request.option_id_for(RuntimePermissionOptionKind::RejectAlways),
                Some(EXPECTED_REJECT_ALWAYS_OPTION_ID.to_string())
            );
            request.request_id
        }
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept allow response");
    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn allow_always_reuses_the_rule_without_emitting_another_request() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("pending request should accept session allow response");
    let first_decision = first.await.expect("first permission task should finish");
    assert_eq!(first_decision, ToolPermissionDecision::Allow);

    let second_decision = tokio::time::timeout(
        Duration::from_millis(100),
        handler.request_permission(permission_request(), &cancellation),
    )
    .await
    .expect("matching session allow should not wait for another response");
    assert_eq!(second_decision, ToolPermissionDecision::Allow);
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn session_rule_mismatch_emits_a_new_permission_request() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("pending request should accept session allow response");
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Allow
    );

    let different_request = ToolPermissionRequest::new(
        ToolCall::new(
            "edit-other",
            "edit",
            serde_json::json!({
                "path": "TEMP.md",
                "old_text": "body",
                "new_text": "updated",
            }),
        ),
        ToolDefinition::new("edit")
            .with_label("Edit")
            .with_kind(ToolKind::Edit)
            .with_permission_policy(ToolPermissionPolicy::Ask),
    );
    let second_handler = Arc::clone(&handler);
    let second_cancellation = cancellation.clone();
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(different_request, &second_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected a new permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("different request should remain approvable once");
    assert_eq!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn reject_always_reuses_the_rule_without_emitting_another_request() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(REJECT_ALWAYS_OPTION_ID.to_string()))
        .expect("pending request should accept session reject response");
    assert!(matches!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));

    let second_decision = tokio::time::timeout(
        Duration::from_millis(100),
        handler.request_permission(permission_request(), &cancellation),
    )
    .await
    .expect("matching session reject should not wait for another response");
    assert!(matches!(
        second_decision,
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn allow_once_does_not_persist_across_requests() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept once allow response");
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Allow
    );

    let second_handler = Arc::clone(&handler);
    let second_cancellation = cancellation.clone();
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(permission_request(), &second_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected a new permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(REJECT_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept once reject response");
    assert!(matches!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
}

#[tokio::test]
async fn cancellation_does_not_persist_a_session_rule() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    cancellation.cancel();
    assert!(matches!(
        first
            .await
            .expect("cancelled permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(
        broker
            .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
            .is_err(),
        "cancelled request should no longer accept a response"
    );

    let second_cancellation = CancellationToken::new();
    let second_handler = Arc::clone(&handler);
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(permission_request(), &second_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected a new permission request after cancellation, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("new request should accept a once response");
    assert_eq!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn clearing_permission_context_forces_matching_requests_to_ask_again() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("pending request should accept session allow response");
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Allow
    );

    broker.clear_permission_context();

    let second_handler = Arc::clone(&handler);
    let second_cancellation = cancellation.clone();
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(permission_request(), &second_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected a new permission request after clearing, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(REJECT_ONCE_OPTION_ID.to_string()))
        .expect("pending request should accept once reject response");
    assert!(matches!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
}

#[tokio::test]
async fn clearing_permission_context_denies_a_delivered_but_unprocessed_response() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender.clone()));
    let cancellation = CancellationToken::new();

    let first_handler = Arc::clone(&handler);
    let first_cancellation = cancellation.clone();
    let first = tokio::spawn(async move {
        first_handler
            .request_permission(permission_request(), &first_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("response should be delivered before the permission task is resumed");
    broker.clear_permission_context();
    assert_eq!(
        first.await.expect("first permission task should finish"),
        ToolPermissionDecision::Deny {
            message: "Tool permission denied: write user rejected the tool call".to_string()
        }
    );

    let second_handler = Arc::clone(&handler);
    let second_cancellation = CancellationToken::new();
    let second = tokio::spawn(async move {
        second_handler
            .request_permission(permission_request(), &second_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected a new permission request after clearing, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("new request should accept a once response");
    assert_eq!(
        second.await.expect("second permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn cancelling_pending_permission_denies_a_delivered_session_response() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();
    let decision = tokio::spawn(async move {
        task_handler
            .request_permission(permission_request(), &task_cancellation)
            .await
    });
    let request_id = match recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("response should be delivered before cancellation");
    broker.cancel_all();

    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Deny {
            message: "Tool permission denied: write user rejected the tool call".to_string()
        }
    );
    assert!(
        broker.rules_guard().is_empty(),
        "cancelling a request must not persist a response that was already delivered"
    );
}

#[test]
fn allow_always_denies_when_context_changes_while_inserting_the_rule() {
    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let cancellation = CancellationToken::new();

    let task_handler = Arc::clone(&handler);
    let task_cancellation = cancellation.clone();
    let decision = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize")
            .block_on(task_handler.request_permission(permission_request(), &task_cancellation))
    });
    let request_id = match receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("permission request should be emitted")
    {
        ConversationEvent::PermissionRequested { request } => request.request_id,
        other => panic!("expected permission request event, got {other:?}"),
    };

    let rules_guard = broker.rules_guard();
    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("response should reach the permission task");
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !decision.is_finished(),
        "permission task should be waiting to insert the session rule"
    );

    broker
        .context_generation
        .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    drop(rules_guard);

    assert_eq!(
        decision.join().expect("permission task should finish"),
        ToolPermissionDecision::Deny {
            message: "Tool permission denied: write user rejected the tool call".to_string()
        }
    );
    assert!(broker.rules_guard().is_empty());
}

#[test]
fn rule_insertion_rejects_a_cancelled_turn_at_the_locked_boundary() {
    let broker = ConversationPermissionBroker::default();
    let cancellation = CancellationToken::new();
    let generation = broker.context_generation();
    let rule =
        ToolPermissionRule::from_request(&permission_request(), ToolPermissionRuleBehavior::Allow)
            .expect("valid permission request should create a rule");
    let rules_guard = broker.rules_guard();
    let insertion_broker = broker.clone();
    let insertion_cancellation = cancellation.clone();
    let (attempt_sender, attempt_receiver) = mpsc::channel();
    let insertion = std::thread::spawn(move || {
        attempt_sender
            .send(())
            .expect("test should announce the insertion attempt");
        insertion_broker.insert_rule_if_active(generation, &insertion_cancellation, rule)
    });

    attempt_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("rule insertion should reach the locked boundary");

    cancellation.cancel();
    drop(rules_guard);

    assert!(
        !insertion.join().expect("rule insertion should finish"),
        "a cancelled turn must not insert a reusable permission rule"
    );
    assert!(broker.rules_guard().is_empty());
}

mod tool_loop_integration;
