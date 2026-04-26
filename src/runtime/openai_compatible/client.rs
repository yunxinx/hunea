use std::{env, time::Duration};

use reqwest::blocking::Client;

use super::{
    ChatCompletionRequestBody, NativeChatRequest, OpenAiCompatibleError,
    collect_chat_completion_stream,
};

const CHAT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// `send_chat_completion` 调用 OpenAI-compatible `/chat/completions` 并聚合流式响应。
pub fn send_chat_completion(request: &NativeChatRequest) -> Result<String, OpenAiCompatibleError> {
    let client = Client::builder()
        .timeout(CHAT_COMPLETION_TIMEOUT)
        .build()
        .map_err(OpenAiCompatibleError::BuildClient)?;
    let endpoint = format!(
        "{}/chat/completions",
        request.base_url.trim_end_matches('/')
    );
    let body = ChatCompletionRequestBody::new(request.model_id.clone(), request.messages.clone());
    let mut builder = client.post(&endpoint).json(&body);
    if let Some(api_key) = request
        .api_key_env
        .as_deref()
        .and_then(|name| env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
    {
        builder = builder.bearer_auth(api_key);
    }

    let response = builder.send().map_err(|_| OpenAiCompatibleError::Request {
        endpoint: endpoint.clone(),
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(OpenAiCompatibleError::Http { endpoint, status });
    }

    collect_chat_completion_stream(response)
}
