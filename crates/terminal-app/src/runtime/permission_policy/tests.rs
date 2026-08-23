use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use runtime_domain::session::RuntimeToolActivityContent;
use tool_runtime::{
    ToolCall, ToolDefinition, ToolKind, ToolPermissionPolicy, ToolPermissionPreview,
};

use super::*;

fn permission_request() -> ToolPermissionRequest {
    ToolPermissionRequest::new(
        ToolCall::new(
            "write-call",
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
        old_text: Some("old\n".to_string()),
        new_text: "new\n".to_string(),
        is_truncated: false,
        snapshot: None,
    })
}

fn interactive_policy(
    notifier: RuntimeEventNotifier,
) -> (PermissionPolicy, ApprovalProviderRegistration) {
    let policy = PermissionPolicy::new(notifier);
    let registration = policy
        .register(
            "terminal-runtime",
            TERMINAL_APPROVAL_PROVIDER_ID,
            Arc::new(InteractiveApprovalProviderFactory),
        )
        .expect("interactive provider should register");
    (policy, registration)
}

async fn recv_request(turn: &PermissionTurn) -> RuntimePermissionRequest {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(request) = turn.try_recv_request() {
                return request;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("permission request should be delivered")
}

async fn request_decision(
    handler: SharedToolPermissionHandler,
    cancellation: CancellationToken,
) -> tokio::task::JoinHandle<ToolPermissionDecision> {
    tokio::spawn(async move {
        handler
            .request_permission(permission_request(), &cancellation)
            .await
    })
}

#[tokio::test]
async fn interactive_provider_queues_delivery_before_wake_and_round_trips_response() {
    let notifier = RuntimeEventNotifier::default();
    let wake_count = Arc::new(AtomicUsize::new(0));
    let observed_wake_count = Arc::clone(&wake_count);
    let _binding = notifier.bind_callback(move || {
        observed_wake_count.fetch_add(1, Ordering::SeqCst);
    });
    let (policy, _registration) = interactive_policy(notifier);
    let turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("turn should open");
    let decision = request_decision(turn.handler(), CancellationToken::new()).await;

    let request = recv_request(&turn).await;
    assert!(wake_count.load(Ordering::SeqCst) > 0);
    assert_eq!(request.title, Some("Write TEMP.md".to_string()));
    assert_eq!(request.options.len(), 4);
    assert_eq!(
        request.option_id_for(RuntimePermissionOptionKind::AllowOnce),
        Some(ALLOW_ONCE_OPTION_ID.to_string())
    );
    assert_eq!(
        request.option_id_for(RuntimePermissionOptionKind::AllowAlways),
        Some(ALLOW_ALWAYS_OPTION_ID.to_string())
    );
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

    turn.respond(&request.request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
        .expect("pending response should route to this turn");
    assert_eq!(
        decision.await.expect("permission task should finish"),
        ToolPermissionDecision::Allow
    );
}

#[tokio::test]
async fn allow_and_reject_rules_are_session_scoped_while_once_is_not() {
    let (policy, _registration) = interactive_policy(RuntimeEventNotifier::default());

    let first_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("first turn should open");
    let first = request_decision(first_turn.handler(), CancellationToken::new()).await;
    let first_request = recv_request(&first_turn).await;
    first_turn
        .respond(
            &first_request.request_id,
            Some(ALLOW_ONCE_OPTION_ID.to_string()),
        )
        .expect("allow once should route");
    assert_eq!(
        first.await.expect("first decision should finish"),
        ToolPermissionDecision::Allow
    );
    drop(first_turn);

    let second_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("second turn should open");
    let second = request_decision(second_turn.handler(), CancellationToken::new()).await;
    let second_request = recv_request(&second_turn).await;
    second_turn
        .respond(
            &second_request.request_id,
            Some(ALLOW_ALWAYS_OPTION_ID.to_string()),
        )
        .expect("allow always should route");
    assert_eq!(
        second.await.expect("second decision should finish"),
        ToolPermissionDecision::Allow
    );
    drop(second_turn);

    let rule_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("rule turn should open");
    let rule_decision = tokio::time::timeout(
        Duration::from_millis(100),
        rule_turn
            .handler()
            .request_permission(permission_request(), &CancellationToken::new()),
    )
    .await
    .expect("matching allow rule should not wait for delivery");
    assert_eq!(rule_decision, ToolPermissionDecision::Allow);
    assert!(rule_turn.try_recv_request().is_none());

    policy.clear_context();
    let reject_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("turn after context clear should open");
    let reject = request_decision(reject_turn.handler(), CancellationToken::new()).await;
    let reject_request = recv_request(&reject_turn).await;
    reject_turn
        .respond(
            &reject_request.request_id,
            Some(REJECT_ALWAYS_OPTION_ID.to_string()),
        )
        .expect("reject always should route");
    assert!(matches!(
        reject.await.expect("reject decision should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    drop(reject_turn);

    let denied_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("stored reject turn should open");
    let denied = tokio::time::timeout(
        Duration::from_millis(100),
        denied_turn
            .handler()
            .request_permission(permission_request(), &CancellationToken::new()),
    )
    .await
    .expect("matching reject rule should not wait for delivery");
    assert!(matches!(denied, ToolPermissionDecision::Deny { .. }));
    assert!(denied_turn.try_recv_request().is_none());
}

#[tokio::test]
async fn cancelling_a_turn_rejects_late_responses_without_clearing_existing_rules() {
    let (policy, _registration) = interactive_policy(RuntimeEventNotifier::default());
    let rule_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("rule turn should open");
    let first = request_decision(rule_turn.handler(), CancellationToken::new()).await;
    let request = recv_request(&rule_turn).await;
    rule_turn
        .respond(
            &request.request_id,
            Some(ALLOW_ALWAYS_OPTION_ID.to_string()),
        )
        .expect("allow always should route");
    assert_eq!(
        first.await.expect("rule decision should finish"),
        ToolPermissionDecision::Allow
    );
    drop(rule_turn);

    let mut pending_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("pending turn should open");
    let different_request = ToolPermissionRequest::new(
        ToolCall::new("bash-call", "bash", serde_json::json!({"command": "pwd"})),
        ToolDefinition::new("bash")
            .with_kind(ToolKind::Execute)
            .with_permission_policy(ToolPermissionPolicy::Ask),
    );
    let cancellation = CancellationToken::new();
    let task_handler = pending_turn.handler();
    let task_cancellation = cancellation.clone();
    let pending = tokio::spawn(async move {
        task_handler
            .request_permission(different_request, &task_cancellation)
            .await
    });
    let pending_request = recv_request(&pending_turn).await;
    // Turn cancellation must close the handler's mutation window even when the
    // worker-level cancellation token has not fired yet.
    pending_turn.cancel_pending();
    assert!(matches!(
        pending.await.expect("cancelled decision should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(matches!(
        pending_turn.respond(
            &pending_request.request_id,
            Some(ALLOW_ALWAYS_OPTION_ID.to_string())
        ),
        Err(PermissionPolicyError::ResponseUnavailable)
    ));
    drop(pending_turn);

    let next_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("next turn should open");
    let decision = next_turn
        .handler()
        .request_permission(permission_request(), &CancellationToken::new())
        .await;
    assert_eq!(decision, ToolPermissionDecision::Allow);
}

#[tokio::test]
async fn context_generation_change_denies_a_delivered_always_response() {
    let (policy, _registration) = interactive_policy(RuntimeEventNotifier::default());
    let turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("turn should open");
    let decision = request_decision(turn.handler(), CancellationToken::new()).await;
    let request = recv_request(&turn).await;
    turn.respond(
        &request.request_id,
        Some(ALLOW_ALWAYS_OPTION_ID.to_string()),
    )
    .expect("response should reach the provider");
    policy.clear_context();

    assert!(matches!(
        decision.await.expect("stale decision should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    let next_turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("new context turn should open");
    let next = request_decision(next_turn.handler(), CancellationToken::new()).await;
    let next_request = recv_request(&next_turn).await;
    next_turn
        .respond(
            &next_request.request_id,
            Some(REJECT_ONCE_OPTION_ID.to_string()),
        )
        .expect("fresh context should ask again");
    assert!(matches!(
        next.await.expect("fresh decision should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
}

#[test]
fn registry_is_transactional_revertible_and_stale_safe() {
    let policy = PermissionPolicy::new(RuntimeEventNotifier::default());
    let factory: Arc<dyn ApprovalProviderFactory> = Arc::new(InteractiveApprovalProviderFactory);
    let mut first = policy
        .register("first-owner", "interactive", Arc::clone(&factory))
        .expect("first provider should register");
    let before = policy.inspection_snapshot();
    let before_next_id = lock(&policy.state).next_registration_id;

    let duplicate = policy
        .register("secret-owner", "interactive", Arc::clone(&factory))
        .expect_err("duplicate should fail before mutation");
    assert!(matches!(
        duplicate,
        PermissionPolicyError::DuplicateProvider { .. }
    ));
    assert_eq!(policy.inspection_snapshot(), before);
    assert_eq!(lock(&policy.state).next_registration_id, before_next_id);

    lock(&policy.state).providers.remove("interactive");
    let _replacement = policy
        .register("replacement-owner", "interactive", factory)
        .expect("replacement should register");
    first.dispose();
    first.dispose();
    assert_eq!(policy.inspection_snapshot().len(), 1);

    {
        let _dropped = policy
            .register(
                "drop-owner",
                "drop-provider",
                Arc::new(InteractiveApprovalProviderFactory),
            )
            .expect("drop provider should register");
        assert_eq!(policy.inspection_snapshot().len(), 2);
    }
    assert_eq!(policy.inspection_snapshot().len(), 1);
}

#[test]
fn deactivation_hides_inspection_rejects_mutation_and_cancels_turns() {
    let (policy, _registration) = interactive_policy(RuntimeEventNotifier::default());
    let mut turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("turn should open");

    policy.deactivate();

    assert!(policy.inspection_snapshot().is_empty());
    assert!(matches!(
        policy.begin_turn(TERMINAL_APPROVAL_PROVIDER_ID),
        Err(PermissionPolicyError::Disposed)
    ));
    assert!(matches!(
        policy.register(
            "owner",
            "next",
            Arc::new(InteractiveApprovalProviderFactory)
        ),
        Err(PermissionPolicyError::Disposed)
    ));
    assert!(turn.try_recv_request().is_none());
    turn.cancel_pending();
}

struct InMemoryApprovalProviderFactory {
    selected_kind: Option<RuntimePermissionOptionKind>,
}

impl ApprovalProviderFactory for InMemoryApprovalProviderFactory {
    fn open(
        &self,
        _notifier: RuntimeEventNotifier,
    ) -> Result<Arc<dyn ApprovalProvider>, ApprovalProviderError> {
        Ok(Arc::new(InMemoryApprovalProvider {
            selected_kind: self.selected_kind,
        }))
    }

    fn adapter_kind(&self) -> &'static str {
        "in-memory"
    }
}

struct InMemoryApprovalProvider {
    selected_kind: Option<RuntimePermissionOptionKind>,
}

impl ApprovalProvider for InMemoryApprovalProvider {
    fn request<'a>(
        &'a self,
        request: RuntimePermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ApprovalFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return None;
            }
            self.selected_kind
                .and_then(|kind| request.option_id_for(kind))
        })
    }

    fn respond(
        &self,
        _request_id: &str,
        _option_id: Option<String>,
    ) -> Result<(), ApprovalProviderError> {
        Err(ApprovalProviderError::Unavailable)
    }

    fn try_recv_request(&self) -> Option<RuntimePermissionRequest> {
        None
    }

    fn cancel_pending(&self) {}
}

#[tokio::test]
async fn in_memory_provider_uses_the_same_begin_turn_handler_contract() {
    let policy = PermissionPolicy::new(RuntimeEventNotifier::default());
    let _registration = policy
        .register(
            "test-owner",
            "in-memory",
            Arc::new(InMemoryApprovalProviderFactory {
                selected_kind: Some(RuntimePermissionOptionKind::AllowOnce),
            }),
        )
        .expect("in-memory provider should register");
    let turn = policy
        .begin_turn("in-memory")
        .expect("in-memory turn should open");

    assert_eq!(
        turn.handler()
            .request_permission(permission_request(), &CancellationToken::new())
            .await,
        ToolPermissionDecision::Allow
    );
    assert!(turn.try_recv_request().is_none());
}

#[tokio::test]
async fn diagnostics_do_not_expose_approval_delivery_or_owner_identity() {
    let secret_owner = "secret-owner-sentinel";
    let secret_path = "secret-path-sentinel";
    let policy = PermissionPolicy::new(RuntimeEventNotifier::default());
    let registration = policy
        .register(
            secret_owner,
            TERMINAL_APPROVAL_PROVIDER_ID,
            Arc::new(InteractiveApprovalProviderFactory),
        )
        .expect("provider should register");
    let turn = policy
        .begin_turn(TERMINAL_APPROVAL_PROVIDER_ID)
        .expect("turn should open");
    let request = ToolPermissionRequest::new(
        ToolCall::new(
            "secret-call",
            "write",
            serde_json::json!({"path": secret_path, "content": "secret-body-sentinel"}),
        ),
        ToolDefinition::new("write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Ask),
    )
    .with_preview(ToolPermissionPreview {
        path: secret_path.to_string(),
        old_text: Some("secret-old-sentinel".to_string()),
        new_text: "secret-new-sentinel".to_string(),
        is_truncated: false,
        snapshot: None,
    });
    let cancellation = CancellationToken::new();
    let task_handler = turn.handler();
    let task_cancellation = cancellation.clone();
    let pending = tokio::spawn(async move {
        task_handler
            .request_permission(request, &task_cancellation)
            .await
    });
    let delivery = recv_request(&turn).await;

    let diagnostic = format!(
        "{policy:?}; {turn:?}; {registration:?}; {:?}",
        policy.inspection_snapshot()
    );
    for secret in [
        secret_owner,
        secret_path,
        "secret-body-sentinel",
        "secret-old-sentinel",
        "secret-new-sentinel",
        "secret-call",
    ] {
        assert!(!diagnostic.contains(secret), "diagnostic leaked {secret}");
    }

    cancellation.cancel();
    assert!(matches!(
        pending.await.expect("cancelled request should finish"),
        ToolPermissionDecision::Deny { .. }
    ));
    assert!(matches!(
        turn.respond(&delivery.request_id, None),
        Err(PermissionPolicyError::ResponseUnavailable)
    ));
}
