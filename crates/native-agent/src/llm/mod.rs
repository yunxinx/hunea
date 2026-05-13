mod client;
mod error;
mod request;

pub use error::NativeLlmError;
pub use request::{ChatMessage, ChatRole, NativeLlmRequest};

pub use client::NativeLlmPerformanceMetrics;
pub(crate) use client::{NativeLlmProgress, client_for_request, model_spec_for_request};
pub(crate) use request::ChatMessageGenAiExt;
