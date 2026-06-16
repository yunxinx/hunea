use conversation_runtime::{
    CancellationToken, ConversationRequest, ProviderKind, ProviderRequest, ProviderRequestError,
    run_conversation_turn_with_cancellation,
};
use provider_protocol::{ConversationItem, Role};
use runtime_domain::session::RuntimeTarget;
use tool_runtime::ToolExecutorRegistry;

#[test]
fn provider_request_carries_provider_kind_and_items() {
    let request = ProviderRequest::new(
        "anthropic",
        ProviderKind::Anthropic,
        "claude-sonnet-4-5",
        None,
        None,
        Some("ANTHROPIC_API_KEY".to_string()),
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "hi"),
        ],
    );

    assert_eq!(request.provider_id, "anthropic");
    assert_eq!(request.provider_kind, ProviderKind::Anthropic);
    assert_eq!(request.model_id, "claude-sonnet-4-5");
    assert_eq!(request.base_url, None);
    assert_eq!(request.items.len(), 2);
}

#[test]
fn provider_request_cancellation_uses_boundary_error_text() {
    assert_eq!(
        ProviderRequestError::Cancelled.to_string(),
        "provider request cancelled"
    );
}

#[test]
fn conversation_request_keeps_model_request_separate_from_tools() {
    let request = ConversationRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ConversationItem::text(Role::User, "summarize src/main.rs")],
    );

    assert_eq!(request.target(), RuntimeTarget::provider("local", "qwen3"));
    assert_eq!(request.provider_request().provider_id, "local");
    assert_eq!(request.provider_request().items.len(), 1);
}

#[tokio::test]
async fn conversation_loop_respects_pre_cancelled_token_before_network_request() {
    let request = ConversationRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ConversationItem::text(Role::User, "hello")],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error = run_conversation_turn_with_cancellation(&request, executor, &cancellation)
        .await
        .expect_err("pre-cancelled request should stop before sending");

    assert_eq!(error.to_string(), "conversation turn cancelled");
}

#[tokio::test]
async fn conversation_loop_respects_pre_cancelled_token_when_tools_are_registered() {
    let request = ConversationRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ConversationItem::text(Role::User, "read Cargo.toml")],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error = run_conversation_turn_with_cancellation(&request, executor, &cancellation)
        .await
        .expect_err("pre-cancelled tool request should stop before sending");

    assert_eq!(error.to_string(), "conversation turn cancelled");
}
