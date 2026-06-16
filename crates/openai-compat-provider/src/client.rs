use futures_util::StreamExt as _;
use provider_protocol::{
    ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderClient,
    ProviderError, ProviderFuture, StreamEventSink,
};

use crate::{
    config::OpenAiClientConfig,
    error_response::provider_error_from_response,
    models::model_descriptors_from_response,
    request::chat_completion_request_body,
    stream::{OpenAiSseDecoder, OpenAiStreamState},
};

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
                .apply_auth(self.http.post(self.config.endpoint("/chat/completions")))
                .json(&body)
                .send()
                .await
                .map_err(|source| ProviderError::Transport(source.to_string()))?;

            if !response.status().is_success() {
                return Err(provider_error_from_response(response).await);
            }

            let mut decoder = OpenAiSseDecoder::default();
            let mut stream_state = OpenAiStreamState::new(request.model.clone());
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
