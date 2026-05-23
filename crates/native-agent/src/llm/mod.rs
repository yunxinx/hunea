mod client;
mod error;
mod provider;
mod request;
mod tool_errors;

pub use error::NativeLlmError;
pub use mo_core::session::NativeLlmPerformanceMetrics;
pub use request::{ChatMessage, ChatRole, NativeLlmRequest};

pub(crate) use client::{
    execute_native_agent_for_execution_request, execute_native_agent_for_request,
};
pub(crate) use provider::{
    list_native_provider_models, openai_client_for_execution_request, openai_client_for_request,
};
pub(crate) use request::{
    prompt_request_from_execution_request, prompt_request_from_native_llm_request,
};
pub(crate) use tool_errors::NativeAgentToolErrorFormatter;

/// `NativeLlmProgress` 描述原生 runtime 流式输出期间可用于 UI 的进度事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLlmProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
}
