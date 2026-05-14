use rig_core::message::Message as RigMessage;

pub use mo_core::session::{ChatMessage, ChatRole, NativeLlmRequest};

pub(crate) fn rig_message_from_chat_message(message: ChatMessage) -> RigMessage {
    match message.role {
        ChatRole::User => RigMessage::user(message.content),
        ChatRole::Assistant => RigMessage::assistant(message.content),
    }
}
