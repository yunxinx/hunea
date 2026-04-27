use std::{
    env, io,
    time::{Duration, Instant},
};

use futures_util::StreamExt as _;
use reqwest::Client;

use super::{
    CancellationToken, ChatCompletionRequestBody, NativeChatRequest, OpenAiCompatibleError,
    stream::collect_chat_completion_stream_chunks_with_delta_handler,
};
use crate::runtime::token_count::StreamingTokenProgress;

const CHAT_COMPLETION_TIMEOUT: Duration = Duration::from_secs(120);

/// `send_chat_completion` 调用 OpenAI-compatible `/chat/completions` 并聚合流式响应。
pub async fn send_chat_completion(
    request: &NativeChatRequest,
) -> Result<String, OpenAiCompatibleError> {
    send_chat_completion_with_cancellation(request, &CancellationToken::default()).await
}

/// `send_chat_completion_with_cancellation` 支持中断 HTTP 请求与流式聚合。
pub async fn send_chat_completion_with_cancellation(
    request: &NativeChatRequest,
    cancellation: &CancellationToken,
) -> Result<String, OpenAiCompatibleError> {
    send_chat_completion_with_cancellation_and_token_progress(request, cancellation, |_| {}).await
}

pub(crate) async fn send_chat_completion_with_cancellation_and_token_progress<F>(
    request: &NativeChatRequest,
    cancellation: &CancellationToken,
    mut on_output_tokens: F,
) -> Result<String, OpenAiCompatibleError>
where
    F: FnMut(usize),
{
    if cancellation.is_cancelled() {
        return Err(OpenAiCompatibleError::Cancelled);
    }

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

    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(OpenAiCompatibleError::Cancelled),
        response = builder.send() => response.map_err(|_| OpenAiCompatibleError::Request {
            endpoint: endpoint.clone(),
        })?,
    };
    let status = response.status();
    if !status.is_success() {
        return Err(OpenAiCompatibleError::Http { endpoint, status });
    }

    let chunks = response
        .bytes_stream()
        .map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(io::Error::other));

    let mut progress = StreamingTokenProgress::new(request.model_id.clone());
    let result =
        collect_chat_completion_stream_chunks_with_delta_handler(chunks, cancellation, |delta| {
            if let Some(total_tokens) = progress.observe_delta(delta, Instant::now()) {
                on_output_tokens(total_tokens);
            }
        })
        .await;
    if result.is_ok()
        && let Some(total_tokens) = progress.flush(Instant::now())
    {
        on_output_tokens(total_tokens);
    }

    result
}
