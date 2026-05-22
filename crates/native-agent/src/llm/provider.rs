use std::{env, time::Duration};

use mo_core::{provider::ProviderKind, session::NativeLlmRequest};
use rig_core::{
    client::{self, ApiKey, DebugExt, ModelListingClient, Provider, ProviderBuilder},
    http_client::{self, HttpClientExt},
    providers::{
        anthropic, cohere, copilot, deepseek, gemini, groq, ollama, openai, together, xai,
        xiaomimimo, zai,
    },
};

use crate::llm::NativeLlmError;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct OpenAiCompatibleExt;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct OpenAiCompatibleBuilder;

#[derive(Debug, Default, Clone)]
pub(crate) struct OptionalBearerAuth(Option<String>);

impl ApiKey for OptionalBearerAuth {
    fn into_header(self) -> Option<http_client::Result<(http::HeaderName, http::HeaderValue)>> {
        self.0
            .filter(|key| !key.trim().is_empty())
            .map(http_client::make_auth_header)
    }
}

impl Provider for OpenAiCompatibleExt {
    type Builder = OpenAiCompatibleBuilder;
    const VERIFY_PATH: &'static str = "/models";
}

impl DebugExt for OpenAiCompatibleExt {}

impl ProviderBuilder for OpenAiCompatibleBuilder {
    type Extension<H>
        = OpenAiCompatibleExt
    where
        H: HttpClientExt;
    type ApiKey = OptionalBearerAuth;

    const BASE_URL: &'static str = "https://api.openai.com/v1";

    fn build<H>(
        _builder: &client::ClientBuilder<Self, Self::ApiKey, H>,
    ) -> http_client::Result<Self::Extension<H>>
    where
        H: HttpClientExt,
    {
        Ok(OpenAiCompatibleExt)
    }
}

type OpenAiCompatibleClient = client::Client<OpenAiCompatibleExt>;
pub(crate) type OpenAiCompatibleModel =
    openai::completion::GenericCompletionModel<OpenAiCompatibleExt>;

pub(crate) fn openai_compatible_model_for_request(
    request: &NativeLlmRequest,
) -> Result<OpenAiCompatibleModel, NativeLlmError> {
    let client = openai_compatible_client_for_request(request)?;
    Ok(openai::completion::GenericCompletionModel::new(
        client,
        request.model_id.clone(),
    ))
}

fn openai_compatible_client_for_request(
    request: &NativeLlmRequest,
) -> Result<OpenAiCompatibleClient, NativeLlmError> {
    let api_key = optional_api_key_for_request(request)?;
    let Some(base_url) = request
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Err(NativeLlmError::MissingBaseUrl {
            provider_id: request.provider_id.clone(),
        });
    };
    OpenAiCompatibleClient::builder()
        .api_key(OptionalBearerAuth(api_key))
        .base_url(normalize_base_url(request, base_url)?)
        .build()
        .map_err(rig_build_error)
}

pub(crate) fn openai_completions_client_for_request(
    request: &NativeLlmRequest,
) -> Result<openai::CompletionsClient, NativeLlmError> {
    let mut builder =
        openai::CompletionsClient::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

fn openai_responses_client_for_request(
    request: &NativeLlmRequest,
) -> Result<openai::Client, NativeLlmError> {
    let mut builder = openai::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn anthropic_client_for_request(
    request: &NativeLlmRequest,
) -> Result<anthropic::Client, NativeLlmError> {
    let mut builder = anthropic::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn gemini_client_for_request(
    request: &NativeLlmRequest,
) -> Result<gemini::Client, NativeLlmError> {
    let mut builder = gemini::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn deepseek_client_for_request(
    request: &NativeLlmRequest,
) -> Result<deepseek::Client, NativeLlmError> {
    let mut builder = deepseek::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn together_client_for_request(
    request: &NativeLlmRequest,
) -> Result<together::Client, NativeLlmError> {
    let mut builder = together::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn groq_client_for_request(
    request: &NativeLlmRequest,
) -> Result<groq::Client, NativeLlmError> {
    let mut builder = groq::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn xai_client_for_request(
    request: &NativeLlmRequest,
) -> Result<xai::Client, NativeLlmError> {
    let mut builder = xai::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn ollama_client_for_request(
    request: &NativeLlmRequest,
) -> Result<ollama::Client, NativeLlmError> {
    let api_key = optional_api_key_for_request(request)?
        .map(ollama::OllamaApiKey::from)
        .unwrap_or_default();
    let mut builder = ollama::Client::builder().api_key(api_key);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn cohere_client_for_request(
    request: &NativeLlmRequest,
) -> Result<cohere::Client, NativeLlmError> {
    let mut builder = cohere::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn zai_client_for_request(
    request: &NativeLlmRequest,
) -> Result<zai::Client, NativeLlmError> {
    let mut builder = zai::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn xiaomi_mimo_client_for_request(
    request: &NativeLlmRequest,
) -> Result<xiaomimimo::Client, NativeLlmError> {
    let mut builder = xiaomimimo::Client::builder().api_key(required_api_key_for_request(request)?);
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
}

pub(crate) fn copilot_client_for_request(
    request: &NativeLlmRequest,
) -> Result<copilot::Client, NativeLlmError> {
    let mut builder = copilot::Client::builder().api_key(copilot::CopilotAuth::ApiKey(
        required_api_key_for_request(request)?,
    ));
    if let Some(base_url) = optional_base_url(request)? {
        builder = builder.base_url(base_url);
    }
    builder.build().map_err(rig_build_error)
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
            list_native_provider_models_inner(request).await
        })
        .await
        .map_err(|_| NativeLlmError::Provider("model sync timed out".to_string()))?
    })
}

async fn list_native_provider_models_inner(
    request: &NativeLlmRequest,
) -> Result<Vec<String>, NativeLlmError> {
    let model_list = match request.provider_kind {
        ProviderKind::OpenAiCompatible => {
            return Err(NativeLlmError::UnsupportedProvider {
                provider_id: request.provider_id.clone(),
                provider_kind: request.provider_kind,
            });
        }
        ProviderKind::OpenAi => openai_responses_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::Anthropic => anthropic_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::Gemini => gemini_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::DeepSeek => deepseek_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::Ollama => ollama_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::Mimo => xiaomi_mimo_client_for_request(request)?
            .list_models()
            .await
            .map_err(rig_model_listing_error)?,
        ProviderKind::Together
        | ProviderKind::Groq
        | ProviderKind::Fireworks
        | ProviderKind::Xai
        | ProviderKind::OllamaCloud
        | ProviderKind::Cohere
        | ProviderKind::Zai
        | ProviderKind::BigModel
        | ProviderKind::Aliyun
        | ProviderKind::Nebius
        | ProviderKind::Vertex
        | ProviderKind::GithubCopilot => {
            return Err(NativeLlmError::UnsupportedProvider {
                provider_id: request.provider_id.clone(),
                provider_kind: request.provider_kind,
            });
        }
    };

    Ok(model_list
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>())
}

fn optional_base_url(request: &NativeLlmRequest) -> Result<Option<String>, NativeLlmError> {
    request
        .base_url
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|base_url| normalize_base_url(request, base_url))
        .transpose()
}

fn normalize_base_url(
    request: &NativeLlmRequest,
    base_url: &str,
) -> Result<String, NativeLlmError> {
    let trimmed = base_url.trim();
    reqwest::Url::parse(trimmed).map_err(|_| NativeLlmError::InvalidBaseUrl {
        provider_id: request.provider_id.clone(),
        base_url: base_url.to_string(),
    })?;
    Ok(trimmed.trim_end_matches('/').to_string())
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

fn rig_build_error(source: http_client::Error) -> NativeLlmError {
    NativeLlmError::Provider(source.to_string())
}

fn rig_model_listing_error(source: rig_core::model::ModelListingError) -> NativeLlmError {
    NativeLlmError::Provider(source.to_string())
}

#[cfg(test)]
mod tests {
    use mo_core::{provider::ProviderKind, session::NativeLlmRequest};

    use super::openai_compatible_client_for_request;
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

        assert!(openai_compatible_client_for_request(&request).is_ok());
    }

    #[test]
    fn openai_completions_provider_requires_api_key() {
        let request = NativeLlmRequest {
            provider_id: "openai".to_string(),
            provider_kind: ProviderKind::OpenAi,
            model_id: "gpt-4o-mini".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
            messages: vec![ChatMessage::user("hello".to_string())],
        };

        let error = super::openai_completions_client_for_request(&request)
            .expect_err("official OpenAI provider should require API key");

        assert!(error.to_string().contains("requires API key"));
    }
}
