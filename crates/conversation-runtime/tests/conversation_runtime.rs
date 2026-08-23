use conversation_runtime::{
    CancellationToken, ConversationRequest, ProviderClientLease, ProviderPromptCachePolicy,
    ProviderRequest, ProviderRequestError, run_conversation_turn_with_cancellation,
};
use provider_protocol::{ConversationItem, Role};
use provider_protocol::{
    ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderClient,
    ProviderError, ProviderFuture, StreamEventSink,
};
use runtime_domain::session::RuntimeTarget;
use std::sync::Arc;
use tool_runtime::ToolExecutorRegistry;

#[test]
fn provider_request_carries_identity_and_items() {
    let request = ProviderRequest::new(
        "anthropic",
        "claude-sonnet-4-5",
        vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "hi"),
        ],
    );

    assert_eq!(request.provider_id, "anthropic");
    assert_eq!(request.model_id, "claude-sonnet-4-5");
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
        "qwen3",
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
        "qwen3",
        vec![ConversationItem::text(Role::User, "hello")],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error =
        run_conversation_turn_with_cancellation(&lease(), &request, executor, &cancellation)
            .await
            .expect_err("pre-cancelled request should stop before sending");

    assert_eq!(error.to_string(), "conversation turn cancelled");
}

#[tokio::test]
async fn conversation_loop_respects_pre_cancelled_token_when_tools_are_registered() {
    let request = ConversationRequest::new(
        "local",
        "qwen3",
        vec![ConversationItem::text(Role::User, "read Cargo.toml")],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error =
        run_conversation_turn_with_cancellation(&lease(), &request, executor, &cancellation)
            .await
            .expect_err("pre-cancelled tool request should stop before sending");

    assert_eq!(error.to_string(), "conversation turn cancelled");
}

fn lease() -> ProviderClientLease {
    ProviderClientLease::new(
        "local",
        runtime_domain::provider::ProviderKind::OpenAiCompatible,
        Arc::new(FakeProvider),
        ProviderPromptCachePolicy::Disabled,
    )
}

struct FakeProvider;

impl ProviderClient for FakeProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        _sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async {
            Err(ProviderError::Transport(
                "test provider not called".to_string(),
            ))
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
