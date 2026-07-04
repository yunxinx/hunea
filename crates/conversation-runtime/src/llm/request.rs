use provider_protocol::PromptRequest;

pub use runtime_domain::session::ProviderRequest;

use crate::conversation::PreparedConversationRequest;
use crate::llm::ProviderRequestError;
use crate::llm::prompt_cache::apply_prompt_cache_options;

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

    let mut prompt_request =
        PromptRequest::new(request.model_id().to_string(), request.items().to_vec());
    apply_prompt_cache_options(&mut prompt_request.options, request);
    Ok(prompt_request)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use provider_protocol::{ContentBlock, ConversationItem, PromptCacheRetention};
    use runtime_domain::session::ConversationTurnRequest;
    use session_store::{InMemorySessionStore, SessionHeader, SessionId};

    use super::ProviderRequest;
    use crate::ProviderKind;
    use crate::{
        ProviderConversation,
        llm::request::{
            prompt_request_from_prepared_request, prompt_request_from_provider_request,
        },
    };

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

    #[test]
    fn prepared_official_openai_request_uses_session_id_as_prompt_cache_key() {
        let header = sample_header();
        let mut conversation = ProviderConversation::with_session_store(
            Arc::new(InMemorySessionStore::new()),
            header.clone(),
        )
        .expect("conversation should initialize");
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "openai",
                ProviderKind::OpenAi,
                "gpt-5-mini",
                None,
                None,
                Some("OPENAI_API_KEY".to_string()),
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        let expected_cache_key = header.session_id.to_string();
        assert_eq!(
            request.options.prompt_cache_key.as_deref(),
            Some(expected_cache_key.as_str())
        );
        assert_eq!(request.options.prompt_cache_retention, None);
    }

    #[test]
    fn prepared_openai_compatible_request_omits_prompt_cache_key() {
        let header = sample_header();
        let mut conversation =
            ProviderConversation::with_session_store(Arc::new(InMemorySessionStore::new()), header)
                .expect("conversation should initialize");
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        assert_eq!(request.options.prompt_cache_key, None);
        assert_eq!(request.options.prompt_cache_retention, None);
    }

    #[test]
    fn prepared_openai_compatible_remote_request_uses_long_prompt_cache_retention() {
        let header = sample_header();
        let mut conversation = ProviderConversation::with_session_store(
            Arc::new(InMemorySessionStore::new()),
            header.clone(),
        )
        .expect("conversation should initialize");
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "remote-compatible",
                ProviderKind::OpenAiCompatible,
                "fast-compatible-model",
                Some("https://compatible.example.com/v1".to_string()),
                None,
                None,
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        let expected_cache_key = header.session_id.to_string();
        assert_eq!(
            request.options.prompt_cache_key.as_deref(),
            Some(expected_cache_key.as_str())
        );
        assert_eq!(
            request.options.prompt_cache_retention,
            Some(PromptCacheRetention::Long24h)
        );
    }

    #[test]
    fn prepared_openai_compatible_direct_openai_request_uses_prompt_cache_key() {
        let header = sample_header();
        let mut conversation = ProviderConversation::with_session_store(
            Arc::new(InMemorySessionStore::new()),
            header.clone(),
        )
        .expect("conversation should initialize");
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "openai-compatible",
                ProviderKind::OpenAiCompatible,
                "gpt-5-mini",
                Some("https://api.openai.com/v1".to_string()),
                None,
                Some("OPENAI_API_KEY".to_string()),
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        let expected_cache_key = header.session_id.to_string();
        assert_eq!(
            request.options.prompt_cache_key.as_deref(),
            Some(expected_cache_key.as_str())
        );
        assert_eq!(request.options.prompt_cache_retention, None);
    }

    #[test]
    fn prepared_openai_responses_request_uses_prompt_cache_key_for_any_base_url() {
        let header = sample_header();
        let mut conversation = ProviderConversation::with_session_store(
            Arc::new(InMemorySessionStore::new()),
            header.clone(),
        )
        .expect("conversation should initialize");
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "responses",
                ProviderKind::OpenAiResponses,
                "fast-responses-model",
                Some("https://responses.example.com/v1".to_string()),
                None,
                None,
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        let expected_cache_key = header.session_id.to_string();
        assert_eq!(
            request.options.prompt_cache_key.as_deref(),
            Some(expected_cache_key.as_str())
        );
        assert_eq!(
            request.options.prompt_cache_retention,
            Some(PromptCacheRetention::Long24h)
        );
    }

    #[test]
    fn prepared_new_session_keeps_initial_prompt_cache_key_after_persistence_starts() {
        let header = sample_header();
        let initial_cache_key = header.session_id.to_string();
        let actual_session_id = SessionId::new();
        assert_ne!(actual_session_id.to_string(), initial_cache_key);
        let mut conversation =
            ProviderConversation::with_session_store(Arc::new(InMemorySessionStore::new()), header)
                .expect("conversation should initialize");
        conversation.set_session_id(actual_session_id);
        let prepared = conversation
            .prepare_turn(&ConversationTurnRequest::new(
                "openai",
                ProviderKind::OpenAi,
                "gpt-5-mini",
                None,
                None,
                Some("OPENAI_API_KEY".to_string()),
                ConversationItem::text(provider_protocol::Role::User, "hello"),
            ))
            .expect("turn should prepare");

        let request = prompt_request_from_prepared_request(&prepared).expect("prompt should build");

        assert_eq!(
            request.options.prompt_cache_key.as_deref(),
            Some(initial_cache_key.as_str())
        );
    }

    fn sample_header() -> SessionHeader {
        SessionHeader {
            session_id: SessionId::new(),
            work_dir: PathBuf::from("/tmp/hunea-cache-test"),
            session_name: Some("cache-test".to_string()),
            initial_model: "gpt-5-mini".to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("test".to_string()),
        }
    }
}
