use std::{env, time::Duration};

use openai_compat_provider::{
    DEFAULT_OPENAI_BASE_URL, OpenAiChatCompletionsClient, OpenAiClientConfig,
    OpenAiCompatibleClient, OpenAiResponsesClient,
};
use provider_protocol::ProviderClient;
use runtime_domain::{provider::ProviderKind, session::ProviderRequest};

use crate::PreparedConversationRequest;
use crate::llm::ProviderRequestError;

pub(crate) fn openai_client_for_request(
    request: &ProviderRequest,
) -> Result<OpenAiCompatibleClient, ProviderRequestError> {
    openai_client_from_parts(
        &request.provider_id,
        request.provider_kind,
        request.base_url.as_deref(),
        request.api_key.as_ref(),
        request.api_key_env.as_deref(),
    )
}

pub(crate) fn openai_client_for_prepared_request(
    request: &PreparedConversationRequest,
) -> Result<OpenAiCompatibleClient, ProviderRequestError> {
    openai_client_from_parts(
        request.provider_id(),
        request.provider_kind(),
        request.base_url(),
        request.api_key(),
        request.api_key_env(),
    )
}

fn openai_client_from_parts(
    provider_id: &str,
    provider_kind: ProviderKind,
    base_url: Option<&str>,
    api_key: Option<&runtime_domain::provider::ProviderApiKey>,
    api_key_env: Option<&str>,
) -> Result<OpenAiCompatibleClient, ProviderRequestError> {
    let config = match provider_kind {
        ProviderKind::OpenAiCompatible | ProviderKind::OpenAiResponses => {
            let Some(base_url) = base_url.filter(|value| !value.trim().is_empty()) else {
                return Err(ProviderRequestError::MissingBaseUrl {
                    provider_id: provider_id.to_string(),
                });
            };
            openai_config_for_base_url(
                provider_id,
                base_url,
                optional_api_key_from_parts(provider_id, provider_kind, api_key, api_key_env)?,
            )
        }
        ProviderKind::OpenAi => {
            let base_url = base_url
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(DEFAULT_OPENAI_BASE_URL);
            openai_config_for_base_url(
                provider_id,
                base_url,
                Some(required_api_key_from_parts(
                    provider_id,
                    provider_kind,
                    api_key,
                    api_key_env,
                )?),
            )
        }
        provider_kind => {
            return Err(ProviderRequestError::UnsupportedProvider {
                provider_id: provider_id.to_string(),
                provider_kind,
            });
        }
    }?;

    match provider_kind {
        ProviderKind::OpenAiResponses => OpenAiResponsesClient::new(config)
            .map(OpenAiCompatibleClient::Responses)
            .map_err(Into::into),
        ProviderKind::OpenAiCompatible | ProviderKind::OpenAi => {
            OpenAiChatCompletionsClient::new(config)
                .map(OpenAiCompatibleClient::ChatCompletions)
                .map_err(Into::into)
        }
        provider_kind => Err(ProviderRequestError::UnsupportedProvider {
            provider_id: provider_id.to_string(),
            provider_kind,
        }),
    }
}

fn openai_config_for_base_url(
    provider_id: &str,
    base_url: &str,
    api_key: Option<String>,
) -> Result<OpenAiClientConfig, ProviderRequestError> {
    OpenAiClientConfig::new(base_url, api_key).map_err(|_| ProviderRequestError::InvalidBaseUrl {
        provider_id: provider_id.to_string(),
        base_url: base_url.to_string(),
    })
}

pub(crate) fn list_provider_models(
    request: &ProviderRequest,
) -> Result<Vec<String>, ProviderRequestError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| {
            ProviderRequestError::Provider(format!("start model sync runtime: {source}"))
        })?;

    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            let client = openai_client_for_request(request)?;
            let models = client.list_models().await?;
            Ok(models.into_iter().map(|model| model.id).collect::<Vec<_>>())
        })
        .await
        .map_err(|_| ProviderRequestError::Provider("model sync timed out".to_string()))?
    })
}

fn optional_api_key_from_parts(
    provider_id: &str,
    provider_kind: ProviderKind,
    api_key: Option<&runtime_domain::provider::ProviderApiKey>,
    api_key_env: Option<&str>,
) -> Result<Option<String>, ProviderRequestError> {
    if let Some(api_key) = api_key {
        return Ok(Some(api_key.as_str().to_string()));
    }
    let Some(api_key_env) = api_key_env.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    env::var(api_key_env)
        .map(|value| value.trim().to_string())
        .map(|value| (!value.is_empty()).then_some(value))
        .map_err(|_| ProviderRequestError::MissingApiKey {
            provider_id: provider_id.to_string(),
            provider_kind,
            api_key_env: Some(api_key_env.to_string()),
        })
}

fn required_api_key_from_parts(
    provider_id: &str,
    provider_kind: ProviderKind,
    api_key: Option<&runtime_domain::provider::ProviderApiKey>,
    api_key_env: Option<&str>,
) -> Result<String, ProviderRequestError> {
    optional_api_key_from_parts(provider_id, provider_kind, api_key, api_key_env)?.ok_or_else(
        || ProviderRequestError::MissingApiKey {
            provider_id: provider_id.to_string(),
            provider_kind,
            api_key_env: api_key_env.map(str::to_string),
        },
    )
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ConversationItem, Role};
    use runtime_domain::{provider::ProviderKind, session::ProviderRequest};

    use super::openai_client_for_request;

    fn user_item(text: &str) -> ConversationItem {
        ConversationItem::text(Role::User, text)
    }

    #[test]
    fn openai_compatible_request_keeps_api_key_optional() {
        let request = ProviderRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            api_key: None,
            api_key_env: None,
            items: vec![user_item("hello")],
        };

        assert!(openai_client_for_request(&request).is_ok());
    }

    #[test]
    fn official_openai_requires_api_key() {
        let request = ProviderRequest {
            provider_id: "openai".to_string(),
            provider_kind: ProviderKind::OpenAi,
            model_id: "gpt-4o-mini".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
            items: vec![user_item("hello")],
        };

        let error = openai_client_for_request(&request)
            .expect_err("official OpenAI provider should require API key");

        assert!(error.to_string().contains("requires API key"));
    }

    #[test]
    fn openai_responses_request_uses_responses_client() {
        let request = ProviderRequest {
            provider_id: "responses".to_string(),
            provider_kind: ProviderKind::OpenAiResponses,
            model_id: "fast-responses-model".to_string(),
            base_url: Some("https://responses.example.com/v1".to_string()),
            api_key: None,
            api_key_env: None,
            items: vec![user_item("hello")],
        };

        let client = openai_client_for_request(&request).expect("client should build");

        assert!(matches!(
            client,
            openai_compat_provider::OpenAiCompatibleClient::Responses(_)
        ));
    }

    #[test]
    fn invalid_base_url_preserves_provider_error_kind() {
        let request = ProviderRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("not a url".to_string()),
            api_key: None,
            api_key_env: None,
            items: vec![user_item("hello")],
        };

        let error = openai_client_for_request(&request)
            .expect_err("invalid base URL should fail before building client");

        assert!(matches!(
            error,
            crate::ProviderRequestError::InvalidBaseUrl { provider_id, base_url }
                if provider_id == "local" && base_url == "not a url"
        ));
    }

    #[test]
    fn non_openai_provider_is_unsupported_by_conversation_adapter() {
        let request = ProviderRequest {
            provider_id: "anthropic".to_string(),
            provider_kind: ProviderKind::Anthropic,
            model_id: "claude".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
            items: vec![user_item("hello")],
        };

        let error = openai_client_for_request(&request).expect_err(
            "non OpenAI-compatible provider is unsupported by the conversation adapter",
        );

        assert!(error.to_string().contains("unsupported provider kind"));
    }
}
