use genai::{
    Client, Headers, ModelIden, ModelSpec, ServiceTarget,
    resolver::{AuthData, AuthResolver, Endpoint},
};

use super::{NativeLlmError, NativeLlmRequest};
use crate::provider_kind::ProviderKindGenAiExt;

/// `NativeLlmProgress` 描述原生 runtime 流式输出期间可用于 UI 的进度事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLlmProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
}

pub use mo_core::session::NativeLlmPerformanceMetrics;

pub(crate) fn client_for_request(request: &NativeLlmRequest) -> Client {
    let Some(auth_data) = request_auth_data(request) else {
        return Client::default();
    };

    let auth_resolver = AuthResolver::from_resolver_fn(
        move |_model_iden: ModelIden| -> Result<Option<AuthData>, genai::resolver::Error> {
            Ok(Some(auth_data.clone()))
        },
    );
    Client::builder().with_auth_resolver(auth_resolver).build()
}

fn request_auth_data(request: &NativeLlmRequest) -> Option<AuthData> {
    if let Some(api_key) = request.api_key.as_ref() {
        return Some(AuthData::from_single(api_key.as_str().to_string()));
    }
    request
        .api_key_env
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map(|api_key_env| AuthData::from_env(api_key_env.clone()))
}

pub(crate) fn model_spec_for_request(
    request: &NativeLlmRequest,
) -> Result<ModelSpec, NativeLlmError> {
    let adapter_kind = request.provider_kind.adapter_kind();
    if let Some(base_url) = request
        .base_url
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        let endpoint = Endpoint::from_owned(normalize_base_url(base_url));
        let model = ModelIden::new(adapter_kind, request.model_id.clone());
        let auth = match request_auth_data(request) {
            Some(auth_data) => auth_data,
            None if request.provider_kind.uses_openai_compatible_endpoint() => {
                AuthData::RequestOverride {
                    url: chat_completions_url(&request.provider_id, base_url)?,
                    headers: Headers::default(),
                }
            }
            None => AuthData::None,
        };

        return Ok(ServiceTarget {
            endpoint,
            auth,
            model,
        }
        .into());
    }

    if request.provider_kind.uses_openai_compatible_endpoint() {
        return Err(NativeLlmError::MissingBaseUrl {
            provider_id: request.provider_id.clone(),
        });
    }

    Ok(ModelIden::new(adapter_kind, request.model_id.clone()).into())
}

fn normalize_base_url(base_url: &str) -> String {
    let mut normalized = base_url.trim().to_string();
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

fn chat_completions_url(provider_id: &str, base_url: &str) -> Result<String, NativeLlmError> {
    let normalized = normalize_base_url(base_url);
    let url = reqwest::Url::parse(&normalized)
        .and_then(|url| url.join("chat/completions"))
        .map_err(|_| NativeLlmError::InvalidBaseUrl {
            provider_id: provider_id.to_string(),
            base_url: base_url.to_string(),
        })?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ProviderApiKey, ProviderKind};

    #[test]
    fn openai_compatible_without_api_key_uses_request_override_for_local_servers() {
        let request = NativeLlmRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("hello".to_string())],
        );

        let spec = model_spec_for_request(&request).expect("model spec should build");
        let ModelSpec::Target(target) = spec else {
            panic!("openai-compatible base_url should build a complete target");
        };
        assert_eq!(target.endpoint.base_url(), "http://127.0.0.1:1234/v1/");
        assert_eq!(target.model.model_name.to_string(), "qwen3");
    }

    #[test]
    fn openai_compatible_with_direct_api_key_uses_single_key_auth() {
        let request = NativeLlmRequest::new(
            "remote",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("https://api.example.com/v1".to_string()),
            Some(ProviderApiKey::new("sk-test-direct")),
            None,
            vec![ChatMessage::user("hello".to_string())],
        );

        let spec = model_spec_for_request(&request).expect("model spec should build");
        let ModelSpec::Target(target) = spec else {
            panic!("openai-compatible base_url should build a complete target");
        };
        assert_eq!(
            target.auth.single_key_value().expect("auth should resolve"),
            "sk-test-direct"
        );
    }

    #[test]
    fn native_provider_custom_base_url_uses_provider_adapter_target() {
        let request = NativeLlmRequest::new(
            "anthropic_proxy",
            ProviderKind::Anthropic,
            "claude-sonnet-4-5",
            Some("https://proxy.example.com/anthropic/v1".to_string()),
            None,
            Some("ANTHROPIC_API_KEY".to_string()),
            vec![ChatMessage::user("hello".to_string())],
        );

        let spec = model_spec_for_request(&request).expect("model spec should build");
        let ModelSpec::Target(target) = spec else {
            panic!("native provider custom base_url should build a complete target");
        };
        assert_eq!(
            target.endpoint.base_url(),
            "https://proxy.example.com/anthropic/v1/"
        );
        assert_eq!(
            target.model.adapter_kind,
            genai::adapter::AdapterKind::Anthropic
        );
        assert_eq!(target.model.model_name.to_string(), "claude-sonnet-4-5");
    }
}
