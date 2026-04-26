mod client;
mod error;
mod protocol;
mod stream;

pub use client::send_chat_completion;
pub use error::OpenAiCompatibleError;
pub use protocol::{ChatCompletionMessage, ChatCompletionRequestBody, NativeChatRequest};
pub use stream::collect_chat_completion_stream;
