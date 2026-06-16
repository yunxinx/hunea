use provider_protocol::PromptRequest;

pub use runtime_domain::session::ProviderRequest;

use crate::conversation::PreparedConversationRequest;
use crate::llm::ProviderRequestError;

pub(crate) fn prompt_request_from_provider_request(
    request: &ProviderRequest,
) -> Result<PromptRequest, ProviderRequestError> {
    if request.items.is_empty() {
        return Err(ProviderRequestError::EmptyPrompt {
            provider_id: request.provider_id.clone(),
        });
    }

    Ok(PromptRequest::new(
        request.model_id.clone(),
        request.items.clone(),
    ))
}

pub(crate) fn prompt_request_from_prepared_request(
    request: &PreparedConversationRequest,
) -> Result<PromptRequest, ProviderRequestError> {
    if request.items().is_empty() {
        return Err(ProviderRequestError::EmptyPrompt {
            provider_id: request.provider_id().to_string(),
        });
    }

    Ok(PromptRequest::new(
        request.model_id().to_string(),
        request.items().to_vec(),
    ))
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ContentBlock, ConversationItem};

    use super::ProviderRequest;
    use crate::ProviderKind;
    use crate::llm::request::prompt_request_from_provider_request;

    #[test]
    fn prompt_request_keeps_structured_user_blocks() {
        let request = ProviderRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            api_key: None,
            api_key_env: None,
            items: vec![ConversationItem::user(vec![
                ContentBlock::Text("review ".to_string()),
                ContentBlock::Image {
                    data_base64: "iVBORw==".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                },
            ])],
        };

        let request = prompt_request_from_provider_request(&request).expect("prompt should build");
        let ConversationItem::Message {
            role: provider_protocol::Role::User,
            content,
        } = &request.items[0]
        else {
            panic!("expected user message item");
        };
        assert!(matches!(
            &content[0],
            ContentBlock::Text(text) if text == "review "
        ));
        assert!(matches!(
            &content[1],
            ContentBlock::Image { data_base64, mime_type, .. }
                if data_base64 == "iVBORw==" && mime_type == "image/png"
        ));
    }
}
