use std::{env, time::Duration};

use mo_ai_core::ProviderClient;
use mo_ai_openai::{DEFAULT_OPENAI_BASE_URL, OpenAiChatCompletionsClient, OpenAiClientConfig};
use mo_core::{provider::ProviderKind, session::NativeLlmRequest};

use crate::llm::NativeLlmError;

pub(crate) fn openai_client_for_request(
    request: &NativeLlmRequest,
) -> Result<OpenAiChatCompletionsClient, NativeLlmError> {
    let config = match request.provider_kind {
        ProviderKind::OpenAiCompatible => {
            let Some(base_url) = request
                .base_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            else {
                return Err(NativeLlmError::MissingBaseUrl {
                    provider_id: request.provider_id.clone(),
                });
            };
            openai_config_for_base_url(request, base_url, optional_api_key_for_request(request)?)
        }
        ProviderKind::OpenAi => {
            let base_url = request
                .base_url
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(DEFAULT_OPENAI_BASE_URL);
            openai_config_for_base_url(
                request,
                base_url,
                Some(required_api_key_for_request(request)?),
            )
        }
        provider_kind => {
            return Err(NativeLlmError::UnsupportedProvider {
                provider_id: request.provider_id.clone(),
                provider_kind,
            });
        }
    }?;

    OpenAiChatCompletionsClient::new(config).map_err(Into::into)
}

fn openai_config_for_base_url(
    request: &NativeLlmRequest,
    base_url: &str,
    api_key: Option<String>,
) -> Result<OpenAiClientConfig, NativeLlmError> {
    OpenAiClientConfig::new(base_url, api_key).map_err(|_| NativeLlmError::InvalidBaseUrl {
        provider_id: request.provider_id.clone(),
        base_url: base_url.to_string(),
    })
}

pub(crate) fn list_native_provider_models(
    request: &NativeLlmRequest,
) -> Result<Vec<String>, NativeLlmError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| {
            NativeLlmError::Provider(format!("start model sync runtime: {source}"))
        })?;

    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), async {
            let client = openai_client_for_request(request)?;
            let models = client.list_models().await?;
            Ok(models.into_iter().map(|model| model.id).collect::<Vec<_>>())
        })
        .await
        .map_err(|_| NativeLlmError::Provider("model sync timed out".to_string()))?
    })
}

fn optional_api_key_for_request(
    request: &NativeLlmRequest,
) -> Result<Option<String>, NativeLlmError> {
    if let Some(api_key) = request.api_key.as_ref() {
        return Ok(Some(api_key.as_str().to_string()));
    }
    let Some(api_key_env) = request
        .api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    env::var(api_key_env)
        .map(|value| value.trim().to_string())
        .map(|value| (!value.is_empty()).then_some(value))
        .map_err(|_| NativeLlmError::MissingApiKey {
            provider_id: request.provider_id.clone(),
            provider_kind: request.provider_kind,
            api_key_env: Some(api_key_env.to_string()),
        })
}

fn required_api_key_for_request(request: &NativeLlmRequest) -> Result<String, NativeLlmError> {
    optional_api_key_for_request(request)?.ok_or_else(|| NativeLlmError::MissingApiKey {
        provider_id: request.provider_id.clone(),
        provider_kind: request.provider_kind,
        api_key_env: request.api_key_env.clone(),
    })
}

#[cfg(test)]
mod tests {
    use mo_core::{provider::ProviderKind, session::NativeLlmRequest};

    use super::openai_client_for_request;
    use crate::ChatMessage;

    #[test]
    fn openai_compatible_request_keeps_api_key_optional() {
        let request = NativeLlmRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("http://127.0.0.1:1234/v1".to_string()),
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user("hello".to_string())],
        };

        assert!(openai_client_for_request(&request).is_ok());
    }

    #[test]
    fn official_openai_requires_api_key() {
        let request = NativeLlmRequest {
            provider_id: "openai".to_string(),
            provider_kind: ProviderKind::OpenAi,
            model_id: "gpt-4o-mini".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user("hello".to_string())],
        };

        let error = openai_client_for_request(&request)
            .expect_err("official OpenAI provider should require API key");

        assert!(error.to_string().contains("requires API key"));
    }

    #[test]
    fn invalid_base_url_preserves_native_error_kind() {
        let request = NativeLlmRequest {
            provider_id: "local".to_string(),
            provider_kind: ProviderKind::OpenAiCompatible,
            model_id: "qwen3".to_string(),
            base_url: Some("not a url".to_string()),
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user("hello".to_string())],
        };

        let error = openai_client_for_request(&request)
            .expect_err("invalid base URL should fail before building client");

        assert!(matches!(
            error,
            crate::NativeLlmError::InvalidBaseUrl { provider_id, base_url }
                if provider_id == "local" && base_url == "not a url"
        ));
    }

    #[test]
    fn non_openai_provider_is_unsupported_by_native_adapter() {
        let request = NativeLlmRequest {
            provider_id: "anthropic".to_string(),
            provider_kind: ProviderKind::Anthropic,
            model_id: "claude".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user("hello".to_string())],
        };

        let error = openai_client_for_request(&request)
            .expect_err("non OpenAI-compatible provider is unsupported by the native adapter");

        assert!(
            error
                .to_string()
                .contains("unsupported native provider kind")
        );
    }
}
