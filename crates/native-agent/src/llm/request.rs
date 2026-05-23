use mo_ai_core::PromptRequest;

pub use mo_core::session::{ChatMessage, ChatRole, NativeLlmRequest};

use crate::agent_session::{NativeAgentExecutionRequest, message_from_chat_message};
use crate::llm::NativeLlmError;

pub(crate) fn prompt_request_from_native_llm_request(
    request: &NativeLlmRequest,
) -> Result<PromptRequest, NativeLlmError> {
    if request.messages.is_empty() {
        return Err(NativeLlmError::EmptyPrompt {
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

pub(crate) fn prompt_request_from_execution_request(
    request: &NativeAgentExecutionRequest,
) -> Result<PromptRequest, NativeLlmError> {
    if request.messages().is_empty() {
        return Err(NativeLlmError::EmptyPrompt {
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
    use mo_ai_core::{MessageContent, MessageRole};

    use super::{ChatMessage, NativeLlmRequest, prompt_request_from_native_llm_request};
    use crate::ProviderKind;
    use mo_core::session::ChatMessageBlock;

    #[test]
    fn prompt_request_keeps_structured_user_blocks() {
        let request = NativeLlmRequest {
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

        let request =
            prompt_request_from_native_llm_request(&request).expect("prompt should build");
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
