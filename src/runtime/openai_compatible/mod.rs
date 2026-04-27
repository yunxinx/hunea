mod client;
mod error;
mod protocol;
mod stream;

pub(crate) use client::send_chat_completion_with_cancellation_and_token_progress;
pub use client::{send_chat_completion, send_chat_completion_with_cancellation};
pub use error::OpenAiCompatibleError;
pub use protocol::{ChatCompletionMessage, ChatCompletionRequestBody, NativeChatRequest};
pub use stream::{
    collect_chat_completion_stream, collect_chat_completion_stream_with_cancellation,
};
pub use tokio_util::sync::CancellationToken;
