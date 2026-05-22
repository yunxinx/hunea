use rig_core::{
    OneOrMany,
    message::{
        Audio, AudioMediaType, Document, DocumentMediaType, DocumentSourceKind, Image, ImageDetail,
        ImageMediaType, Message as RigMessage, MimeType, Text, UserContent,
    },
};

pub use mo_core::session::{ChatMessage, ChatMessageBlock, ChatRole, NativeLlmRequest};

pub(crate) fn rig_message_from_chat_message(message: ChatMessage) -> RigMessage {
    match message.role {
        ChatRole::User => match message.blocks {
            Some(blocks) if !blocks.is_empty() => rig_user_message_from_blocks(blocks),
            _ => RigMessage::user(message.content),
        },
        ChatRole::Assistant => RigMessage::assistant(message.content),
    }
}

fn rig_user_message_from_blocks(blocks: Vec<ChatMessageBlock>) -> RigMessage {
    let mut blocks = blocks.into_iter().map(rig_user_content_from_block);
    let first = blocks.next().expect("caller ensures blocks are non-empty");
    let remaining = blocks.collect::<Vec<_>>();
    let content = if remaining.is_empty() {
        OneOrMany::one(first)
    } else {
        let mut items = Vec::with_capacity(remaining.len() + 1);
        items.push(first);
        items.extend(remaining);
        OneOrMany::many(items).expect("first item guarantees non-empty content")
    };

    RigMessage::User { content }
}

fn rig_user_content_from_block(block: ChatMessageBlock) -> UserContent {
    match block {
        ChatMessageBlock::Text(text) => UserContent::Text(Text { text }),
        ChatMessageBlock::Image {
            data_base64,
            mime_type,
            ..
        } => UserContent::Image(Image {
            data: DocumentSourceKind::Base64(data_base64),
            media_type: ImageMediaType::from_mime_type(&mime_type),
            detail: Some(ImageDetail::Auto),
            additional_params: None,
        }),
        ChatMessageBlock::Audio {
            data_base64,
            mime_type,
            ..
        } => UserContent::Audio(Audio {
            data: DocumentSourceKind::Base64(data_base64),
            media_type: AudioMediaType::from_mime_type(&mime_type),
            additional_params: None,
        }),
        ChatMessageBlock::Document {
            data_base64,
            mime_type,
            filename,
            ..
        } => UserContent::Document(Document {
            data: DocumentSourceKind::Base64(data_base64),
            media_type: DocumentMediaType::from_mime_type(&mime_type),
            additional_params: filename.map(|filename| serde_json::json!({ "filename": filename })),
        }),
    }
}

#[cfg(test)]
mod tests {
    use rig_core::message::{DocumentSourceKind, Message, UserContent};

    use super::{ChatMessage, ChatMessageBlock, rig_message_from_chat_message};

    #[test]
    fn rig_message_from_chat_message_keeps_structured_user_blocks() {
        let message = ChatMessage::user_with_blocks(
            "review @assets/sample.png".to_string(),
            Some(vec![
                ChatMessageBlock::Text("review ".to_string()),
                ChatMessageBlock::Image {
                    data_base64: "iVBORw==".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                },
            ]),
        );

        let message = rig_message_from_chat_message(message);
        let Message::User { content } = message else {
            panic!("expected user message");
        };
        let mut items = content.iter();
        assert!(matches!(
            items.next(),
            Some(UserContent::Text(text)) if text.text == "review "
        ));
        match items.next() {
            Some(UserContent::Image(image)) => {
                assert!(
                    matches!(image.data, DocumentSourceKind::Base64(ref data) if data == "iVBORw==")
                );
            }
            other => panic!("expected image block, got {other:?}"),
        }
    }
}
