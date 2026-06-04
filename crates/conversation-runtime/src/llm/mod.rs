mod client;
mod error;
mod provider;
mod request;
mod tool_errors;

pub use error::ProviderRequestError;
pub use runtime_domain::session::ProviderRequestMetrics;

pub(crate) use client::{execute_conversation_request, execute_prepared_conversation_request};
pub(crate) use provider::{
    list_provider_models, openai_client_for_prepared_request, openai_client_for_request,
};
pub(crate) use request::{
    prompt_request_from_prepared_request, prompt_request_from_provider_request,
};
pub(crate) use tool_errors::ConversationToolErrorFormatter;

/// `ProviderProgress` 描述 provider 流式输出期间可用于 UI 的进度事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
}
