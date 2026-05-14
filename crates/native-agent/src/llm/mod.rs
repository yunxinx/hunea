mod client;
mod error;
mod provider;
mod request;
mod stream;
mod tools;

pub use error::NativeLlmError;
pub use request::{ChatMessage, ChatRole, NativeLlmRequest};

pub(crate) use client::execute_rig_agent_for_request;
pub(crate) use provider::list_native_provider_models;
pub(crate) use request::rig_message_from_chat_message;
pub use stream::NativeLlmPerformanceMetrics;
pub(crate) use stream::NativeLlmProgress;
