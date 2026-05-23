use provider_protocol::PromptRequest;

pub use runtime_domain::session::{ChatMessage, ChatRole, ProviderRequest};

use crate::conversation_session::{PreparedConversationRequest, message_from_chat_message};
use crate::llm::ProviderRequestError;

pub(crate) fn prompt_request_from_provider_request(
    request: &ProviderRequest,
) -> Result<PromptRequest, ProviderRequestError> {
    if request.messages.is_empty() {
        return Err(ProviderRequestError::EmptyPrompt {
            provider_id: request.provider_id.clone(),
        });
    }

    Ok(PromptRequest::new(
        request.model_id.clone(),
        request
            .messages
            .iter()
            .cloned()
            .map(message_from_chat_message)
            .collect(),
    ))
}

pub(crate) fn prompt_request_from_prepared_request(
    request: &PreparedConversationRequest,
) -> Result<PromptRequest, ProviderRequestError> {
    if request.messages().is_empty() {
        return Err(ProviderRequestError::EmptyPrompt {
            provider_id: request.provider_id().to_string(),
        });
    }

    Ok(PromptRequest::new(
        request.model_id().to_string(),
        request.messages().to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use provider_protocol::{MessageContent, MessageRole};

    use super::{ChatMessage, ProviderRequest, prompt_request_from_provider_request};
    use crate::ProviderKind;
    use runtime_domain::session::ChatMessageBlock;

    #[test]
    fn prompt_request_keeps_structured_user_blocks() {
        let request = ProviderRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user_with_blocks(
                "review @assets/sample.png".to_string(),
                Some(vec![
                    ChatMessageBlock::Text("review ".to_string()),
                    ChatMessageBlock::Image {
                        data_base64: "iVBORw==".to_string(),
                        mime_type: "image/png".to_string(),
                        uri: None,
                    },
                ]),
            )],
        };

        let request = prompt_request_from_provider_request(&request).expect("prompt should build");
        assert_eq!(request.messages[0].role, MessageRole::User);
        assert!(matches!(
            &request.messages[0].content[0],
            MessageContent::Text(text) if text == "review "
        ));
        assert!(matches!(
            &request.messages[0].content[1],
            MessageContent::Image { data_base64, mime_type, .. }
                if data_base64 == "iVBORw==" && mime_type == "image/png"
        ));
    }
}
