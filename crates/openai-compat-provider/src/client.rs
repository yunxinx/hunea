use futures_util::StreamExt as _;
use provider_protocol::{
    ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderClient,
    ProviderError, ProviderFuture, StreamEventSink,
};

use crate::{
    config::OpenAiClientConfig,
    error_response::provider_error_from_response,
    models::model_descriptors_from_response,
    request::{chat_completion_request_body, responses_request_body},
    stream::{OpenAiResponsesStreamState, OpenAiSseDecoder, OpenAiStreamState},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiSessionAffinityHeaderSet {
    ChatCompletions,
    Responses,
}

fn session_affinity_header_values(
    request: &PromptRequest,
    header_set: OpenAiSessionAffinityHeaderSet,
) -> Vec<(&'static str, &str)> {
    let Some(session_id) = request
        .options
        .prompt_cache_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };

    match header_set {
        OpenAiSessionAffinityHeaderSet::ChatCompletions => vec![
            ("session_id", session_id),
            ("x-client-request-id", session_id),
            ("x-session-affinity", session_id),
        ],
        OpenAiSessionAffinityHeaderSet::Responses => vec![
            ("session_id", session_id),
            ("x-client-request-id", session_id),
        ],
    }
}

fn apply_session_affinity_headers(
    mut request: reqwest::RequestBuilder,
    prompt_request: &PromptRequest,
    header_set: OpenAiSessionAffinityHeaderSet,
) -> reqwest::RequestBuilder {
    for (name, value) in session_affinity_header_values(prompt_request, header_set) {
        request = request.header(name, value);
    }
    request
}

/// `OpenAiChatCompletionsClient` adapts OpenAI-compatible chat completions to `provider-protocol`.
#[derive(Clone, Debug)]
pub struct OpenAiChatCompletionsClient {
    http: reqwest::Client,
    config: OpenAiClientConfig,
}

impl OpenAiChatCompletionsClient {
    /// `new` creates a client with the default reqwest configuration.
    pub fn new(config: OpenAiClientConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|source| ProviderError::Transport(source.to_string()))?;
        Ok(Self { http, config })
    }

    fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.config.api_key.as_deref() {
            Some(api_key) if !api_key.trim().is_empty() => request.bearer_auth(api_key.trim()),
            _ => request,
        }
    }
}

impl ProviderClient for OpenAiChatCompletionsClient {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let body = chat_completion_request_body(request)?;
            let response = self
                .apply_auth(apply_session_affinity_headers(
                    self.http.post(self.config.endpoint("/chat/completions")),
                    request,
                    OpenAiSessionAffinityHeaderSet::ChatCompletions,
                ))
                .json(&body)
                .send()
                .await
                .map_err(|source| ProviderError::Transport(source.to_string()))?;

            if !response.status().is_success() {
                return Err(provider_error_from_response(response).await);
            }

            let mut decoder = OpenAiSseDecoder::default();
            let mut stream_state = OpenAiStreamState::new();
            let mut bytes = response.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|source| ProviderError::Transport(source.to_string()))?;
                let frames = decoder.push(&chunk)?;
                for frame in frames {
                    if frame == "[DONE]" {
                        return stream_state.finish(sink);
                    }
                    stream_state.apply_data_frame(&frame, sink)?;
                }
            }
            for frame in decoder.finish()? {
                if frame == "[DONE]" {
                    return stream_state.finish(sink);
                }
                stream_state.apply_data_frame(&frame, sink)?;
            }

            stream_state.finish(sink)
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async move {
            let response = self
                .apply_auth(self.http.get(self.config.endpoint("/models")))
                .send()
                .await
                .map_err(|source| ProviderError::Transport(source.to_string()))?;

            if !response.status().is_success() {
                return Err(provider_error_from_response(response).await);
            }

            model_descriptors_from_response(response).await
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

/// `OpenAiResponsesClient` adapts OpenAI-compatible Responses API to `provider-protocol`.
#[derive(Clone, Debug)]
pub struct OpenAiResponsesClient {
    http: reqwest::Client,
    config: OpenAiClientConfig,
}

impl OpenAiResponsesClient {
    /// `new` creates a client with the default reqwest configuration.
    pub fn new(config: OpenAiClientConfig) -> Result<Self, ProviderError> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|source| ProviderError::Transport(source.to_string()))?;
        Ok(Self { http, config })
    }

    fn apply_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.config.api_key.as_deref() {
            Some(api_key) if !api_key.trim().is_empty() => request.bearer_auth(api_key.trim()),
            _ => request,
        }
    }
}

impl ProviderClient for OpenAiResponsesClient {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let body = responses_request_body(request)?;
            let response = self
                .apply_auth(apply_session_affinity_headers(
                    self.http.post(self.config.endpoint("/responses")),
                    request,
                    OpenAiSessionAffinityHeaderSet::Responses,
                ))
                .json(&body)
                .send()
                .await
                .map_err(|source| ProviderError::Transport(source.to_string()))?;

            if !response.status().is_success() {
                return Err(provider_error_from_response(response).await);
            }

            let mut decoder = OpenAiSseDecoder::default();
            let mut stream_state = OpenAiResponsesStreamState::default();
            let mut bytes = response.bytes_stream();
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|source| ProviderError::Transport(source.to_string()))?;
                let frames = decoder.push(&chunk)?;
                for frame in frames {
                    if frame == "[DONE]" {
                        return stream_state.finish(sink);
                    }
                    stream_state.apply_data_frame(&frame, sink)?;
                }
            }
            for frame in decoder.finish()? {
                if frame == "[DONE]" {
                    return stream_state.finish(sink);
                }
                stream_state.apply_data_frame(&frame, sink)?;
            }

            stream_state.finish(sink)
        })
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        Box::pin(async move {
            let response = self
                .apply_auth(self.http.get(self.config.endpoint("/models")))
                .send()
                .await
                .map_err(|source| ProviderError::Transport(source.to_string()))?;

            if !response.status().is_success() {
                return Err(provider_error_from_response(response).await);
            }

            model_descriptors_from_response(response).await
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }
}

/// `OpenAiCompatibleClient` selects the OpenAI-compatible API surface configured for a provider.
#[derive(Clone, Debug)]
pub enum OpenAiCompatibleClient {
    ChatCompletions(OpenAiChatCompletionsClient),
    Responses(OpenAiResponsesClient),
}

impl ProviderClient for OpenAiCompatibleClient {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        match self {
            Self::ChatCompletions(client) => client.stream_prompt(request, sink),
            Self::Responses(client) => client.stream_prompt(request, sink),
        }
    }

    fn list_models<'a>(
        &'a self,
    ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
        match self {
            Self::ChatCompletions(client) => client.list_models(),
            Self::Responses(client) => client.list_models(),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        match self {
            Self::ChatCompletions(client) => client.capabilities(),
            Self::Responses(client) => client.capabilities(),
        }
    }
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ConversationItem, PromptRequest, Role};

    use super::{OpenAiSessionAffinityHeaderSet, session_affinity_header_values};

    fn request_with_prompt_cache_key(prompt_cache_key: &str) -> PromptRequest {
        let mut request = PromptRequest::new(
            "fast-compatible-model",
            vec![ConversationItem::text(Role::User, "hello")],
        );
        request.options.prompt_cache_key = Some(prompt_cache_key.to_string());
        request
    }

    #[test]
    fn chat_completions_session_affinity_headers_match_cache_headers() {
        let request = request_with_prompt_cache_key("session-123");

        let headers = session_affinity_header_values(
            &request,
            OpenAiSessionAffinityHeaderSet::ChatCompletions,
        );

        assert_eq!(
            headers,
            vec![
                ("session_id", "session-123"),
                ("x-client-request-id", "session-123"),
                ("x-session-affinity", "session-123"),
            ]
        );
    }

    #[test]
    fn responses_session_affinity_headers_match_cache_headers() {
        let request = request_with_prompt_cache_key("session-123");

        let headers =
            session_affinity_header_values(&request, OpenAiSessionAffinityHeaderSet::Responses);

        assert_eq!(
            headers,
            vec![
                ("session_id", "session-123"),
                ("x-client-request-id", "session-123"),
            ]
        );
    }

    #[test]
    fn session_affinity_headers_ignore_blank_prompt_cache_key() {
        let request = request_with_prompt_cache_key("  ");

        let headers = session_affinity_header_values(
            &request,
            OpenAiSessionAffinityHeaderSet::ChatCompletions,
        );

        assert!(headers.is_empty());
    }
}
