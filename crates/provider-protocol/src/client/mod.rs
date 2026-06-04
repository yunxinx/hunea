use std::{future::Future, pin::Pin};

use crate::{
    error::ProviderError,
    model::{ModelDescriptor, ProviderCapabilities},
    prompt::{PromptCompletion, PromptRequest},
    stream::StreamEvent,
};

/// `ProviderFuture` is the boxed async return type used by provider clients.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// `StreamEventSink` consumes provider-neutral streaming events.
pub trait StreamEventSink {
    /// `emit` forwards one streaming event to the runtime.
    fn emit(&mut self, event: StreamEvent);
}

impl<F> StreamEventSink for F
where
    F: FnMut(StreamEvent),
{
    fn emit(&mut self, event: StreamEvent) {
        self(event);
    }
}

/// `ProviderClient` is a single-turn model caller, not an agent runtime.
pub trait ProviderClient: Send + Sync {
    /// `stream_prompt` sends one provider request and emits normalized stream events.
    fn stream_prompt<'a>(
        &'a self,
        request: PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>>;

    /// `list_models` returns provider model ids.
    fn list_models<'a>(&'a self)
    -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>>;

    /// `capabilities` describes the supported provider adapter features.
    fn capabilities(&self) -> ProviderCapabilities;
}
