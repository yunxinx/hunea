use thiserror::Error;

use crate::ToolCall;

/// provider-neutral 角色。
///
/// `Tool` 角色不再存在，工具结果通过 `ConversationItem::ToolResult` 表达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// 返回协议层角色标签。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// provider-neutral 内容承载单元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentBlock {
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
        size: Option<u64>,
    },
    ResourceText {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    ToolCall(ToolCall),
}

impl ContentBlock {
    /// 返回文本内容（仅 `Text` variant）。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }

    /// 返回 tool call 内容（仅 `ToolCall` variant）。
    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match self {
            Self::ToolCall(call) => Some(call),
            _ => None,
        }
    }
}

/// 拼接 content blocks 中 provider 可见的文本内容。
pub fn visible_text_from_blocks(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            ContentBlock::ResourceText { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>()
}

/// provider-neutral 协议的原子语义单元。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationItem {
    /// 消息，承载文本、多模态内容与 assistant tool call。
    Message {
        role: Role,
        content: Vec<ContentBlock>,
    },
    /// 工具执行结果，通过 call_id 关联。
    ToolResult {
        call_id: String,
        content: Vec<ContentBlock>,
        is_error: bool,
    },
    /// 模型推理内容，部分场景需回传 provider。
    Reasoning {
        content: String,
        summary: Option<String>,
        encrypted: Option<String>,
    },
}

/// 单个 `ConversationItem` 的语义校验错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConversationItemValidationError {
    /// `ToolCall` 只能出现在 assistant message 中。
    #[error("tool call content is only valid on assistant messages, found in {role:?} message")]
    ToolCallOutsideAssistant { role: Role },
    /// `ToolResult` item 不能再嵌套 `ToolCall` content block。
    #[error("tool call content is only valid on assistant messages, found in ToolResult item")]
    ToolCallInToolResult,
}

impl ConversationItem {
    /// 创建 system 消息。
    pub fn system(content: Vec<ContentBlock>) -> Self {
        Self::message(Role::System, content)
    }

    /// 创建 user 消息。
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self::message(Role::User, content)
    }

    /// 创建 assistant 消息。
    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Self::message(Role::Assistant, content)
    }

    /// 创建 assistant 消息，并附带 tool call。
    pub fn assistant_with_parts(mut content: Vec<ContentBlock>, tool_calls: Vec<ToolCall>) -> Self {
        content.extend(tool_calls.into_iter().map(ContentBlock::ToolCall));
        Self::assistant(content)
    }

    /// 创建单文本消息。
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        let text = text.into();
        let content = if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text(text)]
        };
        Self::message(role, content)
    }

    /// 创建带 tool call 的 assistant 消息。
    pub fn assistant_with_tool_calls(text: String, tool_calls: Vec<ToolCall>) -> Self {
        let content = if text.is_empty() {
            Vec::new()
        } else {
            vec![ContentBlock::Text(text)]
        };
        Self::assistant_with_parts(content, tool_calls)
    }

    /// 根据 role 创建消息。
    pub fn message(role: Role, content: Vec<ContentBlock>) -> Self {
        Self::Message { role, content }
    }

    /// 创建工具结果 item。
    pub fn tool_result(
        call_id: impl Into<String>,
        content: Vec<ContentBlock>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            call_id: call_id.into(),
            content,
            is_error,
        }
    }

    /// 创建推理 item。
    pub fn reasoning(content: impl Into<String>) -> Self {
        Self::Reasoning {
            content: content.into(),
            summary: None,
            encrypted: None,
        }
    }

    /// 返回消息 content blocks（非消息 item 返回 None）。
    pub fn content_blocks(&self) -> Option<&[ContentBlock]> {
        match self {
            Self::Message { content, .. } => Some(content.as_slice()),
            _ => None,
        }
    }

    /// 返回可见文本内容（Text + ResourceText），排除 Reasoning。
    pub fn text_content(&self) -> String {
        let blocks = match self {
            Self::Message { content, .. } | Self::ToolResult { content, .. } => content,
            Self::Reasoning { .. } => return String::new(),
        };
        visible_text_from_blocks(blocks)
    }

    /// 返回 assistant 消息中的 tool call。
    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        let blocks = match self {
            Self::Message {
                role: Role::Assistant,
                content,
            } => content.as_slice(),
            _ => &[],
        };
        blocks.iter().filter_map(ContentBlock::as_tool_call)
    }

    /// 返回消息角色（非消息 item 返回 None）。
    pub const fn role(&self) -> Option<Role> {
        match self {
            Self::Message { role, .. } => Some(*role),
            _ => None,
        }
    }

    /// 校验单个 item 的语义合法性。
    pub fn validate(&self) -> Result<(), ConversationItemValidationError> {
        match self {
            Self::Message { role, content }
                if *role != Role::Assistant
                    && content.iter().any(|block| block.as_tool_call().is_some()) =>
            {
                Err(ConversationItemValidationError::ToolCallOutsideAssistant { role: *role })
            }
            Self::ToolResult { content, .. }
                if content.iter().any(|block| block.as_tool_call().is_some()) =>
            {
                Err(ConversationItemValidationError::ToolCallInToolResult)
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ToolCall;

    use super::{ContentBlock, ConversationItem, Role};

    #[test]
    fn role_as_str() {
        assert_eq!(Role::System.as_str(), "system");
        assert_eq!(Role::User.as_str(), "user");
        assert_eq!(Role::Assistant.as_str(), "assistant");
    }

    #[test]
    fn content_block_as_text() {
        assert_eq!(ContentBlock::Text("hello".into()).as_text(), Some("hello"));
        assert_eq!(
            ContentBlock::Image {
                data_base64: "x".into(),
                mime_type: "image/png".into(),
                uri: None,
            }
            .as_text(),
            None,
        );
    }

    #[test]
    fn item_text_non_empty() {
        let item = ConversationItem::text(Role::User, "hello");
        assert_eq!(
            item,
            ConversationItem::Message {
                role: Role::User,
                content: vec![ContentBlock::Text("hello".into())],
            },
        );
    }

    #[test]
    fn item_text_empty_yields_no_blocks() {
        let item = ConversationItem::text(Role::User, "");
        assert_eq!(
            item,
            ConversationItem::Message {
                role: Role::User,
                content: vec![],
            },
        );
    }

    #[test]
    fn assistant_message_stores_tool_calls_in_content() {
        let item = ConversationItem::assistant_with_tool_calls(
            "thinking".into(),
            vec![ToolCall::new("c1", "bash", r#"{"cmd":"ls"}"#)],
        );
        match &item {
            ConversationItem::Message { role, content } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(content.len(), 2);
                assert!(matches!(&content[0], ContentBlock::Text(t) if t == "thinking"));
                assert!(
                    matches!(&content[1], ContentBlock::ToolCall(call) if call.call_id == "c1" && call.name == "bash")
                );
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn item_assistant_tool_calls_are_read_from_content_blocks() {
        let call = ToolCall::new("c1", "bash", "{}");
        let item = ConversationItem::assistant_with_tool_calls(String::new(), vec![call.clone()]);

        assert_eq!(item.tool_calls().cloned().collect::<Vec<_>>(), vec![call]);
    }

    #[test]
    fn item_assistant_with_tool_calls_empty_text() {
        let item = ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new("c1", "bash", "{}")],
        );
        match &item {
            ConversationItem::Message { role, content } => {
                assert_eq!(*role, Role::Assistant);
                assert_eq!(content.len(), 1);
                assert!(
                    matches!(&content[0], ContentBlock::ToolCall(call) if call.call_id == "c1")
                );
            }
            _ => panic!("expected Message variant"),
        }
    }

    #[test]
    fn item_tool_result() {
        let item =
            ConversationItem::tool_result("c1", vec![ContentBlock::Text("ok".into())], false);
        assert!(matches!(
            item,
            ConversationItem::ToolResult {
                is_error: false,
                ..
            },
        ));
    }

    #[test]
    fn item_tool_result_error() {
        let item =
            ConversationItem::tool_result("c1", vec![ContentBlock::Text("fail".into())], true);
        assert!(matches!(
            item,
            ConversationItem::ToolResult { is_error: true, .. },
        ));
    }

    #[test]
    fn item_reasoning_defaults() {
        let item = ConversationItem::reasoning("thinking hard");
        assert_eq!(
            item,
            ConversationItem::Reasoning {
                content: "thinking hard".into(),
                summary: None,
                encrypted: None,
            },
        );
    }

    #[test]
    fn item_text_content_message() {
        let item = ConversationItem::assistant(vec![
            ContentBlock::Text("hello ".into()),
            ContentBlock::ResourceText {
                uri: "file:///x".into(),
                mime_type: None,
                text: "world".into(),
            },
        ]);
        assert_eq!(item.text_content(), "hello world");
    }

    #[test]
    fn item_text_content_excludes_reasoning() {
        let item = ConversationItem::Reasoning {
            content: "internal".into(),
            summary: None,
            encrypted: None,
        };
        assert_eq!(item.text_content(), "");
    }

    #[test]
    fn item_text_content_tool_result() {
        let item =
            ConversationItem::tool_result("c1", vec![ContentBlock::Text("result".into())], false);
        assert_eq!(item.text_content(), "result");
    }

    #[test]
    fn item_tool_calls_only_for_assistant() {
        let assistant = ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![
                ToolCall::new("c1", "bash", "{}"),
                ToolCall::new("c2", "read", r#"{"path":"x"}"#),
            ],
        );
        assert_eq!(assistant.tool_calls().count(), 2);

        let user = ConversationItem::message(
            Role::User,
            vec![ContentBlock::ToolCall(ToolCall::new(
                "c3",
                "read",
                "{}".to_string(),
            ))],
        );
        assert_eq!(user.tool_calls().count(), 0);

        let tool_result = ConversationItem::tool_result("c1", vec![], false);
        assert_eq!(tool_result.tool_calls().count(), 0);

        let reasoning = ConversationItem::reasoning("thinking");
        assert_eq!(reasoning.tool_calls().count(), 0);
    }

    #[test]
    fn item_role_accessor() {
        assert_eq!(
            ConversationItem::text(Role::System, "x").role(),
            Some(Role::System)
        );
        assert_eq!(
            ConversationItem::text(Role::User, "x").role(),
            Some(Role::User)
        );
        assert_eq!(
            ConversationItem::assistant_with_tool_calls(String::new(), vec![]).role(),
            Some(Role::Assistant),
        );
        assert_eq!(
            ConversationItem::tool_result("c1", vec![], false).role(),
            None
        );
        assert_eq!(ConversationItem::reasoning("x").role(), None);
    }

    #[test]
    fn tool_calls_are_extracted_only_from_assistant_messages() {
        let system = ConversationItem::system(vec![ContentBlock::Text("prompt".into())]);
        let user = ConversationItem::user(vec![
            ContentBlock::Text("review".into()),
            ContentBlock::ToolCall(ToolCall::new("c1", "read", "{}")),
            ContentBlock::Image {
                data_base64: "x".into(),
                mime_type: "image/png".into(),
                uri: None,
            },
        ]);

        assert_eq!(system.role(), Some(Role::System));
        assert_eq!(user.role(), Some(Role::User));
        assert_eq!(system.tool_calls().count(), 0);
        assert_eq!(user.tool_calls().count(), 0);
    }

    #[test]
    fn validate_rejects_tool_call_in_user_message() {
        let item = ConversationItem::user(vec![ContentBlock::ToolCall(ToolCall::new(
            "c1",
            "bash",
            "{}".to_string(),
        ))]);

        let error = item
            .validate()
            .expect_err("user ToolCall should be invalid");

        assert_eq!(
            error,
            super::ConversationItemValidationError::ToolCallOutsideAssistant { role: Role::User }
        );
    }
}
