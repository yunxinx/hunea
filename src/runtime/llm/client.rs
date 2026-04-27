use std::time::Instant;

use futures_util::StreamExt as _;
use genai::{
    Client, Headers, ModelIden, ModelSpec, ServiceTarget,
    chat::{ChatOptions, ChatRequest, ChatStreamEvent},
    resolver::{AuthData, AuthResolver, Endpoint},
};

use super::{LlmError, NativeChatRequest};
use crate::runtime::token_count::StreamingTokenProgress;

/// `send_chat` 通过 genai 发起流式请求，并在完成后返回聚合文本。
pub async fn send_chat(request: &NativeChatRequest) -> Result<String, LlmError> {
    send_chat_with_cancellation(request, &tokio_util::sync::CancellationToken::default()).await
}

/// `send_chat_with_cancellation` 支持中断请求与流式聚合。
pub async fn send_chat_with_cancellation(
    request: &NativeChatRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<String, LlmError> {
    send_chat_with_cancellation_and_token_progress(request, cancellation, |_| {}).await
}

pub(crate) async fn send_chat_with_cancellation_and_token_progress<F>(
    request: &NativeChatRequest,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_output_tokens: F,
) -> Result<String, LlmError>
where
    F: FnMut(usize),
{
    if cancellation.is_cancelled() {
        return Err(LlmError::Cancelled);
    }

    let client = client_for_request(request);
    let chat_request = ChatRequest::new(
        request
            .messages
            .clone()
            .into_iter()
            .map(|message| message.into_genai())
            .collect(),
    );
    let model = model_spec_for_request(request)?;
    let options = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_usage(true);

    let stream_response = tokio::select! {
        _ = cancellation.cancelled() => return Err(LlmError::Cancelled),
        response = client.exec_chat_stream(model, chat_request, Some(&options)) => response?,
    };

    let mut stream = stream_response.stream;
    let mut content = String::new();
    let mut progress = StreamingTokenProgress::new(request.model_id.clone());

    loop {
        let event = tokio::select! {
            _ = cancellation.cancelled() => return Err(LlmError::Cancelled),
            event = stream.next() => event,
        };
        let Some(event) = event else {
            break;
        };

        match event? {
            ChatStreamEvent::Start => {}
            ChatStreamEvent::Chunk(chunk) => {
                content.push_str(&chunk.content);
                if let Some(total_tokens) = progress.observe_delta(&chunk.content, Instant::now()) {
                    on_output_tokens(total_tokens);
                }
            }
            ChatStreamEvent::ReasoningChunk(_)
            | ChatStreamEvent::ThoughtSignatureChunk(_)
            | ChatStreamEvent::ToolCallChunk(_) => {}
            ChatStreamEvent::End(end) => {
                if content.is_empty()
                    && let Some(captured) = end.captured_content
                    && let Some(captured_text) = captured.joined_texts()
                {
                    content = captured_text;
                }
                break;
            }
        }
    }

    if let Some(total_tokens) = progress.flush(Instant::now()) {
        on_output_tokens(total_tokens);
    }

    Ok(content)
}

fn client_for_request(request: &NativeChatRequest) -> Client {
    let Some(api_key_env) = request
        .api_key_env
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
    else {
        return Client::default();
    };

    let auth_resolver = AuthResolver::from_resolver_fn(
        move |_model_iden: ModelIden| -> Result<Option<AuthData>, genai::resolver::Error> {
            Ok(Some(AuthData::from_env(api_key_env.clone())))
        },
    );
    Client::builder().with_auth_resolver(auth_resolver).build()
}

fn model_spec_for_request(request: &NativeChatRequest) -> Result<ModelSpec, LlmError> {
    let adapter_kind = request.provider_kind.adapter_kind();
    if let Some(base_url) = request
        .base_url
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        let endpoint = Endpoint::from_owned(normalize_base_url(base_url));
        let model = ModelIden::new(adapter_kind, request.model_id.clone());
        let auth = match request
            .api_key_env
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            Some(env_name) => AuthData::from_env(env_name.clone()),
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
        return Err(LlmError::MissingBaseUrl {
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

fn chat_completions_url(provider_id: &str, base_url: &str) -> Result<String, LlmError> {
    let normalized = normalize_base_url(base_url);
    let url = reqwest::Url::parse(&normalized)
        .and_then(|url| url.join("chat/completions"))
        .map_err(|_| LlmError::InvalidBaseUrl {
            provider_id: provider_id.to_string(),
            base_url: base_url.to_string(),
        })?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::llm::{ChatMessage, ProviderKind};

    #[test]
    fn openai_compatible_without_api_key_uses_request_override_for_local_servers() {
        let request = NativeChatRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
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
    fn native_provider_custom_base_url_uses_provider_adapter_target() {
        let request = NativeChatRequest::new(
            "anthropic_proxy",
            ProviderKind::Anthropic,
            "claude-sonnet-4-5",
            Some("https://proxy.example.com/anthropic/v1".to_string()),
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
