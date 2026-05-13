use genai::chat::ChatMessage as GenAiChatMessage;

pub use mo_core::session::{ChatMessage, ChatRole, NativeLlmRequest};

pub(crate) trait ChatMessageGenAiExt {
    fn into_genai(self) -> GenAiChatMessage;
}

impl ChatMessageGenAiExt for ChatMessage {
    fn into_genai(self) -> GenAiChatMessage {
        match self.role {
            ChatRole::User => GenAiChatMessage::user(self.content),
            ChatRole::Assistant => GenAiChatMessage::assistant(self.content),
        }
    }
}
