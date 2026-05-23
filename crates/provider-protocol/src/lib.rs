//! Provider-neutral AI request, message, stream, and tool contracts.

pub mod client;
pub mod error;
pub mod message;
pub mod model;
pub mod prompt;
pub mod stream;
pub mod tool;

pub use client::{ProviderClient, ProviderFuture, StreamEventSink};
pub use error::ProviderError;
pub use message::{Message, MessageContent, MessageRole};
pub use model::{ModelDescriptor, ProviderCapabilities};
pub use prompt::{FinishReason, PromptOptions, PromptRequest, PromptResponse, TokenUsage};
pub use stream::StreamEvent;
pub use tool::{ToolCall, ToolDefinition, ToolResult};
