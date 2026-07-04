//! OpenAI-compatible `/v1/chat/completions` provider adapter.

mod client;
mod config;
mod error_response;
mod models;
mod request;
mod stream;

pub use client::{OpenAiChatCompletionsClient, OpenAiCompatibleClient, OpenAiResponsesClient};
pub use config::{DEFAULT_OPENAI_BASE_URL, OpenAiClientConfig};
pub use request::{
    OpenAiRequestFormat, PromptRequestProjection, prompt_request_projection,
    prompt_request_projection_for_format, prompt_request_projection_from_parts,
    prompt_request_projection_from_parts_for_format,
};
