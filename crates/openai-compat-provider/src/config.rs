use std::time::Duration;

use provider_protocol::ProviderError;

pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// `OpenAiClientConfig` stores endpoint, optional bearer auth, and the request
/// idle timeout for OpenAI-compatible APIs.
///
/// `idle_timeout` 约束单次 HTTP 交互：建连等待与响应数据块之间的空闲间隔；
/// 收到数据即重置，不限制整个请求的总时长。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiClientConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub idle_timeout: Duration,
}

impl OpenAiClientConfig {
    /// `new` validates and normalizes an OpenAI-compatible base URL.
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        idle_timeout: Duration,
    ) -> Result<Self, ProviderError> {
        let base_url = normalize_base_url(base_url.into())?;
        Ok(Self {
            base_url,
            api_key,
            idle_timeout,
        })
    }

    /// `official_openai` creates a config for the official OpenAI API endpoint.
    pub fn official_openai(api_key: String, idle_timeout: Duration) -> Result<Self, ProviderError> {
        Self::new(DEFAULT_OPENAI_BASE_URL, Some(api_key), idle_timeout)
    }

    pub(crate) fn endpoint(&self, path: &str) -> String {
        format!("{}/{}", self.base_url, path.trim_start_matches('/'))
    }

    pub(crate) fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.api_key.as_deref() {
            Some(api_key) if !api_key.trim().is_empty() => request.bearer_auth(api_key.trim()),
            _ => request,
        }
    }
}

fn normalize_base_url(base_url: String) -> Result<String, ProviderError> {
    let trimmed = base_url.trim();
    reqwest::Url::parse(trimmed).map_err(|_| {
        ProviderError::Protocol(format!("invalid OpenAI-compatible base_url {base_url:?}"))
    })?;
    Ok(trimmed.trim_end_matches('/').to_string())
}
