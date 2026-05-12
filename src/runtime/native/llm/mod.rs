mod client;
mod error;
mod request;

pub use error::NativeLlmError;
pub use request::{ChatMessage, ChatRole, NativeLlmRequest};

pub(crate) use client::{
    NativeLlmPerformanceMetrics, NativeLlmProgress, client_for_request, model_spec_for_request,
};
