use lumos::runtime::{
    native::{
        CancellationToken, ChatMessage, NativeAgentRequest, NativeChatError, NativeChatRequest,
        ProviderKind, send_agent_turn_with_cancellation,
    },
    session::RuntimeTarget,
    tools::{RuntimeToolDefinition, RuntimeToolRegistry, ToolPermissionPolicy},
};

#[test]
fn native_chat_request_carries_provider_kind_and_messages() {
    let request = NativeChatRequest::new(
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
    assert_eq!(NativeChatError::Cancelled.to_string(), "chat cancelled");
}

#[test]
fn native_agent_request_keeps_model_request_and_tools_separate() {
    let mut tools = RuntimeToolRegistry::new();
    tools.insert(
        RuntimeToolDefinition::new("read_file")
            .with_label("Read file")
            .with_description("Read a UTF-8 text file from the workspace")
            .with_input_schema(serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }))
            .with_permission_policy(ToolPermissionPolicy::Ask),
    );

    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ChatMessage::user("summarize src/main.rs".to_string())],
    )
    .with_tools(tools);

    assert_eq!(
        request.target(),
        RuntimeTarget::native_agent("local", "qwen3")
    );
    assert_eq!(request.chat_request().provider_id, "local");
    assert_eq!(request.chat_request().messages.len(), 1);
    assert_eq!(
        request
            .tools()
            .definition("read_file")
            .expect("tool should be registered")
            .permission_policy,
        ToolPermissionPolicy::Ask
    );
}

#[tokio::test]
async fn native_agent_turn_respects_pre_cancelled_token_before_network_request() {
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

    let error = send_agent_turn_with_cancellation(&request, &cancellation)
        .await
        .expect_err("pre-cancelled request should stop before sending");

    assert_eq!(error.to_string(), "agent turn cancelled");
}

#[tokio::test]
async fn native_agent_turn_rejects_tools_until_executor_is_attached() {
    let mut tools = RuntimeToolRegistry::new();
    tools.insert(RuntimeToolDefinition::new("read_file"));
    let request = NativeAgentRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        vec![ChatMessage::user("read Cargo.toml".to_string())],
    )
    .with_tools(tools);

    let error = send_agent_turn_with_cancellation(&request, &CancellationToken::default())
        .await
        .expect_err("tools must not be silently ignored");

    assert_eq!(error.to_string(), "native agent tools require an executor");
}
