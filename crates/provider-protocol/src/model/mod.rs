/// `ModelDescriptor` is the stable model-listing shape exposed by provider adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub id: String,
}

impl ModelDescriptor {
    /// `new` creates a model descriptor from a provider model id.
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

/// `ProviderCapabilities` describes the adapter features runtime code may rely on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub has_streaming: bool,
    pub has_tools: bool,
    pub has_model_listing: bool,
}

impl ProviderCapabilities {
    /// `chat_completions` returns the baseline OpenAI-compatible capabilities.
    pub const fn chat_completions() -> Self {
        Self {
            has_streaming: true,
            has_tools: true,
            has_model_listing: true,
        }
    }
}
