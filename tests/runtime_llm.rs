use lumos::runtime::llm::{ChatMessage, NativeChatRequest, ProviderKind};

#[test]
fn native_chat_request_carries_provider_kind_and_messages() {
    let request = NativeChatRequest::new(
        "anthropic",
        ProviderKind::Anthropic,
        "claude-sonnet-4-5",
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
