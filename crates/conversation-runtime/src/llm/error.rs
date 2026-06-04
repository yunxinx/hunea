use std::fmt;

use provider_protocol::ProviderError;
use runtime_domain::provider::ProviderKind;

/// `ProviderRequestError` 描述 provider 请求失败。
#[derive(Debug)]
pub enum ProviderRequestError {
    MissingBaseUrl {
        provider_id: String,
    },
    EmptyPrompt {
        provider_id: String,
    },
    MissingApiKey {
        provider_id: String,
        provider_kind: ProviderKind,
        api_key_env: Option<String>,
    },
    InvalidBaseUrl {
        provider_id: String,
        base_url: String,
    },
    UnsupportedProvider {
        provider_id: String,
        provider_kind: ProviderKind,
    },
    Provider(String),
    Cancelled,
}

impl fmt::Display for ProviderRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBaseUrl { provider_id } => {
                write!(f, "provider {provider_id} requires base_url")
            }
            Self::EmptyPrompt { provider_id } => {
                write!(f, "provider {provider_id} received no prompt items")
            }
            Self::MissingApiKey {
                provider_id,
                provider_kind,
                api_key_env,
            } => match api_key_env {
                Some(api_key_env) => write!(
                    f,
                    "provider {provider_id} ({provider_kind}) requires API key from {api_key_env}"
                ),
                None => write!(
                    f,
                    "provider {provider_id} ({provider_kind}) requires API key"
                ),
            },
            Self::InvalidBaseUrl {
                provider_id,
                base_url,
            } => write!(
                f,
                "provider {provider_id} has invalid base_url {base_url:?}"
            ),
            Self::UnsupportedProvider {
                provider_id,
                provider_kind,
            } => write!(
                f,
                "provider {provider_id} uses unsupported provider kind {provider_kind}"
            ),
            Self::Provider(message) => write!(f, "{message}"),
            Self::Cancelled => write!(f, "provider request cancelled"),
        }
    }
}

impl std::error::Error for ProviderRequestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

impl From<ProviderError> for ProviderRequestError {
    fn from(source: ProviderError) -> Self {
        Self::Provider(source.to_string())
    }
}
