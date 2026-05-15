use mo_core::session::RuntimeTarget;
use mo_native_agent::{
    CancellationToken, ChatMessage, NativeAgentRequest, NativeLlmError, NativeLlmRequest,
    ProviderKind, send_agent_loop_with_cancellation,
};
use mo_tools::ToolExecutorRegistry;

#[test]
fn native_llm_request_carries_provider_kind_and_messages() {
    let request = NativeLlmRequest::new(
        "anthropic",
        ProviderKind::Anthropic,
        "claude-sonnet-4-5",
        None,
        None,
        Some("ANTHROPIC_API_KEY".to_string()),
        vec![
            ChatMessage::user("hello".to_string()),
            ChatMessage::assistant("hi".to_string()),
        ],
    );

    assert_eq!(request.provider_id, "anthropic");
    assert_eq!(request.provider_kind, ProviderKind::Anthropic);
    assert_eq!(request.model_id, "claude-sonnet-4-5");
    assert_eq!(request.base_url, None);
    assert_eq!(request.messages.len(), 2);
}

#[test]
fn runtime_exposes_native_as_named_boundary() {
    assert_eq!(
        NativeLlmError::Cancelled.to_string(),
        "native LLM request cancelled"
    );
}

#[test]
fn native_agent_request_keeps_model_request_separate_from_tools() {
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ChatMessage::user("summarize src/main.rs".to_string())],
    );

    assert_eq!(
        request.target(),
        RuntimeTarget::native_agent("local", "qwen3")
    );
    assert_eq!(request.llm_request().provider_id, "local");
    assert_eq!(request.llm_request().messages.len(), 1);
}

#[tokio::test]
async fn native_agent_loop_respects_pre_cancelled_token_before_network_request() {
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ChatMessage::user("hello".to_string())],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error = send_agent_loop_with_cancellation(&request, executor, &cancellation)
        .await
        .expect_err("pre-cancelled request should stop before sending");

    assert_eq!(error.to_string(), "agent turn cancelled");
}

#[tokio::test]
async fn native_agent_loop_respects_pre_cancelled_token_when_tools_are_registered() {
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ChatMessage::user("read Cargo.toml".to_string())],
    );
    let cancellation = CancellationToken::default();
    cancellation.cancel();

    let executor = ToolExecutorRegistry::new();
    let error = send_agent_loop_with_cancellation(&request, executor, &cancellation)
        .await
        .expect_err("pre-cancelled tool request should stop before sending");

    assert_eq!(error.to_string(), "agent turn cancelled");
}
