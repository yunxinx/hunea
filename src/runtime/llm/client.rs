use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use genai::{
    Client, Headers, ModelIden, ModelSpec, ServiceTarget,
    chat::{ChatOptions, ChatRequest, ChatStreamEvent},
    resolver::{AuthData, AuthResolver, Endpoint},
};

use super::{LlmError, NativeChatRequest};
use crate::runtime::token_count::StreamingTokenProgress;

/// `NativeChatResponse` 保存原生 runtime 的正文与可选 reasoning 内容。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeChatResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
}

/// `NativeChatProgress` 描述原生 runtime 流式输出期间可用于 UI 的进度事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeChatProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
}

/// `send_chat` 通过 genai 发起流式请求，并在完成后返回聚合文本。
pub async fn send_chat(request: &NativeChatRequest) -> Result<String, LlmError> {
    send_chat_with_cancellation(request, &tokio_util::sync::CancellationToken::default())
        .await
        .map(|response| response.content)
}

/// `send_chat_with_cancellation` 支持中断请求与流式聚合。
pub async fn send_chat_with_cancellation(
    request: &NativeChatRequest,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<NativeChatResponse, LlmError> {
    send_chat_with_cancellation_and_token_progress(request, cancellation, |_| {}).await
}

pub(crate) async fn send_chat_with_cancellation_and_token_progress<F>(
    request: &NativeChatRequest,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_progress: F,
) -> Result<NativeChatResponse, LlmError>
where
    F: FnMut(NativeChatProgress),
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
        .with_capture_reasoning_content(true)
        .with_capture_usage(true);

    let stream_response = tokio::select! {
        _ = cancellation.cancelled() => return Err(LlmError::Cancelled),
        response = client.exec_chat_stream(model, chat_request, Some(&options)) => response?,
    };

    let mut stream = stream_response.stream;
    let mut output = NativeChatAccumulator::new(request.model_id.clone());

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
                output.observe_content_chunk(&chunk.content, Instant::now(), &mut on_progress);
            }
            ChatStreamEvent::ReasoningChunk(chunk) => {
                output.observe_reasoning_chunk(&chunk.content, Instant::now(), &mut on_progress);
            }
            ChatStreamEvent::ThoughtSignatureChunk(_) | ChatStreamEvent::ToolCallChunk(_) => {}
            ChatStreamEvent::End(end) => {
                if output.content.is_empty()
                    && let Some(captured) = end.captured_content
                    && let Some(captured_text) = captured.joined_texts()
                {
                    output.content = captured_text;
                }
                if output.reasoning_content.is_empty()
                    && let Some(captured_reasoning) = end.captured_reasoning_content
                {
                    output.reasoning_content = captured_reasoning;
                }
                break;
            }
        }
    }

    if let Some(total_tokens) = output.progress.flush(Instant::now()) {
        on_progress(NativeChatProgress::OutputTokens { total_tokens });
    }

    Ok(output.finish())
}

struct NativeChatAccumulator {
    content: String,
    reasoning_content: String,
    progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
}

impl NativeChatAccumulator {
    fn new(model_id: String) -> Self {
        Self {
            content: String::new(),
            reasoning_content: String::new(),
            progress: StreamingTokenProgress::new(model_id),
            is_thinking: false,
            reasoning_started_at: None,
            reasoning_finished_at: None,
        }
    }

    fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeChatProgress),
    ) {
        if content.is_empty() {
            return;
        }
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(NativeChatProgress::Thinking { is_thinking: false });
        }
        self.content.push_str(content);
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeChatProgress),
    ) {
        if content.is_empty() {
            return;
        }
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(NativeChatProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_token_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeChatProgress),
    ) {
        if let Some(total_tokens) = self.progress.observe_delta(content, now) {
            on_progress(NativeChatProgress::OutputTokens { total_tokens });
        }
    }

    fn finish(self) -> NativeChatResponse {
        let reasoning_content = trim_outer_blank_lines(&self.reasoning_content);
        let reasoning_duration = self.reasoning_duration();
        let content = if reasoning_content.is_empty() {
            self.content
        } else {
            trim_outer_blank_lines(&self.content)
        };
        NativeChatResponse {
            content,
            reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
            reasoning_duration,
        }
    }

    fn reasoning_duration(&self) -> Option<Duration> {
        if self.reasoning_content.trim().is_empty() {
            return None;
        }

        let started_at = self.reasoning_started_at?;
        let finished_at = self.reasoning_finished_at.unwrap_or(started_at);
        Some(finished_at.saturating_duration_since(started_at))
    }
}

fn trim_outer_blank_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("start exists when at least one non-blank line exists");

    lines[start..=end].join("\n")
}

fn client_for_request(request: &NativeChatRequest) -> Client {
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

fn request_auth_data(request: &NativeChatRequest) -> Option<AuthData> {
    if let Some(api_key) = request.api_key.as_ref() {
        return Some(AuthData::from_single(api_key.as_str().to_string()));
    }
    request
        .api_key_env
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map(|api_key_env| AuthData::from_env(api_key_env.clone()))
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
    use crate::runtime::llm::{ChatMessage, ProviderApiKey, ProviderKind};

    #[test]
    fn openai_compatible_without_api_key_uses_request_override_for_local_servers() {
        let request = NativeChatRequest::new(
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
        let request = NativeChatRequest::new(
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
        let request = NativeChatRequest::new(
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

    #[test]
    fn native_chat_accumulator_tracks_reasoning_tokens_and_body_separately() {
        let started_at = Instant::now();
        let mut accumulator = NativeChatAccumulator::new("qwen/qwen3-4b-thinking-2507".to_string());
        let mut progress = Vec::new();

        accumulator.observe_reasoning_chunk("先分析问题。", started_at, &mut |event| {
            progress.push(event)
        });
        accumulator.observe_reasoning_chunk(
            "再给出答案。",
            started_at + std::time::Duration::from_millis(120),
            &mut |event| progress.push(event),
        );
        accumulator.observe_content_chunk(
            "最终答案",
            started_at + std::time::Duration::from_millis(240),
            &mut |event| progress.push(event),
        );

        let response = accumulator.finish();

        assert_eq!(response.content, "最终答案");
        assert_eq!(
            response.reasoning_content,
            Some("先分析问题。再给出答案。".to_string())
        );
        assert_eq!(
            response.reasoning_duration,
            Some(std::time::Duration::from_millis(240))
        );
        assert_eq!(
            progress.first(),
            Some(&NativeChatProgress::Thinking { is_thinking: true })
        );
        assert!(progress.iter().any(|event| {
            matches!(event, NativeChatProgress::OutputTokens { total_tokens } if *total_tokens > 0)
        }));
        assert!(progress.contains(&NativeChatProgress::Thinking { is_thinking: false }));
    }

    #[test]
    fn native_chat_accumulator_hides_empty_reasoning() {
        let accumulator = NativeChatAccumulator::new("qwen3".to_string());

        let response = accumulator.finish();

        assert_eq!(response.reasoning_content, None);
    }

    #[test]
    fn native_chat_accumulator_trims_reasoning_boundary_blank_lines() {
        let mut accumulator = NativeChatAccumulator::new("qwen3".to_string());
        let started_at = Instant::now();

        accumulator.observe_reasoning_chunk("先分析\n\n", started_at, &mut |_| {});
        accumulator.observe_content_chunk("\n\n结论", started_at, &mut |_| {});

        let response = accumulator.finish();

        assert_eq!(response.reasoning_content, Some("先分析".to_string()));
        assert_eq!(response.content, "结论");
    }
}
