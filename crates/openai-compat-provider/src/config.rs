use provider_protocol::ProviderError;

pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// `OpenAiClientConfig` stores endpoint and optional bearer auth for OpenAI-compatible APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiClientConfig {
    pub base_url: String,
    pub api_key: Option<String>,
}

impl OpenAiClientConfig {
    /// `new` validates and normalizes an OpenAI-compatible base URL.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
    ) -> Result<Self, ProviderError> {
        let base_url = normalize_base_url(base_url.into())?;
        Ok(Self { base_url, api_key })
    }

    /// `official_openai` creates a config for the official OpenAI API endpoint.
    pub fn official_openai(api_key: String) -> Result<Self, ProviderError> {
        Self::new(DEFAULT_OPENAI_BASE_URL, Some(api_key))
    }

    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }
}

fn normalize_base_url(base_url: String) -> Result<String, ProviderError> {
    let trimmed = base_url.trim();
    reqwest::Url::parse(trimmed).map_err(|_| {
        ProviderError::Protocol(format!("invalid OpenAI-compatible base_url {base_url:?}"))
    })?;
    Ok(trimmed.trim_end_matches('/').to_string())
}
