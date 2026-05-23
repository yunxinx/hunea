use crate::tool::{ToolCall, ToolResult};

/// `MessageRole` is the provider-neutral role set used across provider runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    /// `as_str` returns the protocol-common role label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// `MessageContent` is the unified multimodal content block used before provider projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageContent {
    Text(String),
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
    },
    Document {
        data_base64: String,
        mime_type: String,
        filename: Option<String>,
        uri: Option<String>,
    },
    ResourceLink {
        name: String,
        uri: String,
        mime_type: Option<String>,
        size: Option<i64>,
    },
    ResourceText {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Reasoning(String),
}

/// `Message` is the workspace-internal message representation, independent of provider JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<MessageContent>,
}

impl Message {
    /// `new` creates a message from explicit content blocks.
    pub fn new(role: MessageRole, content: Vec<MessageContent>) -> Self {
        Self { role, content }
    }

    /// `text` creates a single-text-block message.
    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        let text = text.into();
        let content = if text.is_empty() {
            Vec::new()
        } else {
            vec![MessageContent::Text(text)]
        };
        Self { role, content }
    }

    /// `assistant_with_tool_calls` creates an assistant message that requested tools.
    pub fn assistant_with_tool_calls(text: String, tool_calls: Vec<ToolCall>) -> Self {
        let mut content = Vec::new();
        if !text.is_empty() {
            content.push(MessageContent::Text(text));
        }
        content.extend(tool_calls.into_iter().map(MessageContent::ToolCall));
        Self {
            role: MessageRole::Assistant,
            content,
        }
    }

    /// `tool_result` creates a provider-context tool result message.
    pub fn tool_result(result: ToolResult) -> Self {
        Self {
            role: MessageRole::Tool,
            content: vec![MessageContent::ToolResult(result)],
        }
    }

    /// `text_content` returns visible text/resource text blocks concatenated in order.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|content| match content {
                MessageContent::Text(text) => Some(text.as_str()),
                MessageContent::ResourceText { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// `tool_calls` returns assistant tool calls in message order.
    pub fn tool_calls(&self) -> Vec<ToolCall> {
        self.content
            .iter()
            .filter_map(|content| match content {
                MessageContent::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }

    /// `tool_result` returns the first tool result block if this is a tool message.
    pub fn first_tool_result(&self) -> Option<&ToolResult> {
        self.content.iter().find_map(|content| match content {
            MessageContent::ToolResult(result) => Some(result),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Message, MessageContent, MessageRole};

    #[test]
    fn text_content_keeps_reasoning_out_of_visible_text() {
        let message = Message::new(
            MessageRole::Assistant,
            vec![
                MessageContent::Reasoning("hidden chain".to_string()),
                MessageContent::Text("visible answer".to_string()),
            ],
        );

        assert_eq!(message.text_content(), "visible answer");
    }
}
