use mo_ai_core::{Message, MessageContent, MessageRole};
use mo_core::session::{
    ChatMessage, ChatMessageBlock, ChatRole, NativeAgentTurnRequest, RuntimeTarget,
};

use crate::{ProviderApiKey, ProviderKind};

/// `NativeAgentSessionError` 描述 native agent 会话组装失败。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum NativeAgentSessionError {
    #[error("Native agent turn request must carry a user message")]
    NonUserTurnMessage,
    #[error("Native agent session already has a pending user turn")]
    PendingTurnAlreadyActive,
}

/// `NativeAgentExecutionRequest` 是 native runtime 实际执行时使用的完整请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAgentExecutionRequest {
    provider_id: String,
    provider_kind: ProviderKind,
    model_id: String,
    base_url: Option<String>,
    api_key: Option<ProviderApiKey>,
    api_key_env: Option<String>,
    messages: Vec<Message>,
}

impl NativeAgentExecutionRequest {
    /// `new` 创建一次 native agent 完整执行请求。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            messages,
        }
    }

    /// `target` 返回执行请求的统一 runtime 目标。
    pub fn target(&self) -> RuntimeTarget {
        RuntimeTarget::native_agent(self.provider_id.clone(), self.model_id.clone())
    }

    /// `provider_id` 返回 provider 标识。
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// `provider_kind` 返回 provider 类型。
    pub const fn provider_kind(&self) -> ProviderKind {
        self.provider_kind
    }

    /// `model_id` 返回模型标识。
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// `base_url` 返回 provider base_url。
    pub fn base_url(&self) -> Option<&str> {
        self.base_url.as_deref()
    }

    /// `api_key` 返回直接配置的 API key。
    pub fn api_key(&self) -> Option<&ProviderApiKey> {
        self.api_key.as_ref()
    }

    /// `api_key_env` 返回 API key 环境变量名。
    pub fn api_key_env(&self) -> Option<&str> {
        self.api_key_env.as_deref()
    }

    /// `messages` 返回 provider-visible 完整消息历史。
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

/// `NativeAgentSession` 持有 native agent 的 provider-visible 内存会话。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeAgentSession {
    system_prompt: Option<String>,
    history: Vec<Message>,
    pending_user_message: Option<Message>,
}

impl NativeAgentSession {
    /// `new` 创建空的 native agent 会话。
    pub fn new() -> Self {
        Self::default()
    }

    /// `clear` 清空当前会话。
    pub fn clear(&mut self) {
        self.history.clear();
        self.pending_user_message = None;
    }

    /// `truncate_after_user_turns` 保留指定数量的已提交 user turns。
    pub fn truncate_after_user_turns(&mut self, retained_user_turns: usize) {
        self.pending_user_message = None;
        if retained_user_turns == 0 {
            self.history.clear();
            return;
        }

        let mut user_turn_count = 0usize;
        let mut truncate_index = self.history.len();
        for (index, message) in self.history.iter().enumerate() {
            if message.role != MessageRole::User {
                continue;
            }
            user_turn_count = user_turn_count.saturating_add(1);
            if user_turn_count > retained_user_turns {
                truncate_index = index;
                break;
            }
        }
        self.history.truncate(truncate_index);
    }

    /// `set_system_prompt` 设置会话级 system prompt。
    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt.and_then(normalize_system_prompt);
    }

    /// `system_prompt` 返回当前生效的 system prompt。
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// `history` 返回当前 provider-visible 历史。
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// `prepare_turn` 接受一个 native 用户 turn，并构造完整执行请求。
    pub fn prepare_turn(
        &mut self,
        turn: &NativeAgentTurnRequest,
    ) -> Result<NativeAgentExecutionRequest, NativeAgentSessionError> {
        if turn.message().role != ChatRole::User {
            return Err(NativeAgentSessionError::NonUserTurnMessage);
        }
        if self.pending_user_message.is_some() {
            return Err(NativeAgentSessionError::PendingTurnAlreadyActive);
        }

        let user_message = message_from_chat_message(turn.message().clone());
        self.pending_user_message = Some(user_message.clone());

        Ok(NativeAgentExecutionRequest::new(
            turn.provider_id().to_string(),
            turn.provider_kind(),
            turn.model_id().to_string(),
            turn.base_url().map(str::to_string),
            turn.api_key().cloned(),
            turn.api_key_env().map(str::to_string),
            self.provider_messages_with_pending_user(&user_message),
        ))
    }

    /// `commit_pending_user` 把已开始发送给 provider 的当前用户消息写回会话历史。
    pub fn commit_pending_user(&mut self) -> bool {
        let Some(user_message) = self.pending_user_message.take() else {
            return false;
        };
        self.history.push(user_message);
        true
    }

    /// `rollback_pending_user` 丢弃尚未开始发送给 provider 的当前用户消息。
    pub fn rollback_pending_user(&mut self) -> bool {
        self.pending_user_message.take().is_some()
    }

    /// `commit_turn_messages` 把 runtime 生成的 provider-visible 消息写回会话历史。
    pub fn commit_turn_messages(&mut self, messages: impl IntoIterator<Item = Message>) {
        self.history.extend(messages);
    }

    fn provider_messages(&self) -> Vec<Message> {
        let mut messages =
            Vec::with_capacity(self.history.len() + usize::from(self.system_prompt.is_some()));
        if let Some(system_prompt) = self.system_prompt.as_deref() {
            messages.push(Message::text(MessageRole::System, system_prompt));
        }
        messages.extend(self.history.iter().cloned());
        messages
    }

    fn provider_messages_with_pending_user(&self, user_message: &Message) -> Vec<Message> {
        let mut messages = self.provider_messages();
        messages.push(user_message.clone());
        messages
    }
}

pub(crate) fn message_from_chat_message(message: ChatMessage) -> Message {
    let role = match message.role {
        ChatRole::User => MessageRole::User,
        ChatRole::Assistant => MessageRole::Assistant,
    };
    let content = match message.blocks {
        Some(blocks) if !blocks.is_empty() => {
            blocks.into_iter().map(content_from_chat_block).collect()
        }
        _ => vec![MessageContent::Text(message.content)],
    };

    Message::new(role, content)
}

fn content_from_chat_block(block: ChatMessageBlock) -> MessageContent {
    match block {
        ChatMessageBlock::Text(text) => MessageContent::Text(text),
        ChatMessageBlock::Image {
            data_base64,
            mime_type,
            uri,
        } => MessageContent::Image {
            data_base64,
            mime_type,
            uri,
        },
        ChatMessageBlock::Audio {
            data_base64,
            mime_type,
            uri,
        } => MessageContent::Audio {
            data_base64,
            mime_type,
            uri,
        },
        ChatMessageBlock::Document {
            data_base64,
            mime_type,
            filename,
            uri,
        } => MessageContent::Document {
            data_base64,
            mime_type,
            filename,
            uri,
        },
    }
}

fn normalize_system_prompt(prompt: String) -> Option<String> {
    let prompt = prompt.trim().to_string();
    (!prompt.is_empty()).then_some(prompt)
}

#[cfg(test)]
mod tests {
    use mo_ai_core::{Message, MessageContent, MessageRole, ToolCall, ToolResult};
    use serde_json::json;

    use super::{NativeAgentSession, NativeAgentSessionError, message_from_chat_message};
    use crate::{ChatMessage, ProviderKind};
    use mo_core::session::{ChatMessageBlock, NativeAgentTurnRequest};

    #[test]
    fn prepare_turn_uses_session_history_and_current_user_message() {
        let mut session = NativeAgentSession::new();
        session.commit_turn_messages([Message::text(MessageRole::Assistant, "first answer")]);

        let request = session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("follow up".to_string()),
            ))
            .expect("turn should prepare");

        let visible_text = request
            .messages()
            .iter()
            .map(Message::text_content)
            .collect::<Vec<_>>();
        assert_eq!(visible_text, vec!["first answer", "follow up"]);
        assert_eq!(session.history().len(), 1);
    }

    #[test]
    fn prepare_turn_prepends_system_prompt_without_persisting_it_in_history() {
        let mut session = NativeAgentSession::new();
        session.set_system_prompt(Some("You are helpful".to_string()));

        let request = session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("hello".to_string()),
            ))
            .expect("turn should prepare");

        assert_eq!(request.messages()[0].role, MessageRole::System);
        assert_eq!(request.messages()[0].text_content(), "You are helpful");
        assert!(session.history().is_empty());
    }

    #[test]
    fn prepare_turn_rejects_non_user_message() {
        let mut session = NativeAgentSession::new();

        let error = session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::assistant("not a user turn".to_string()),
            ))
            .expect_err("assistant turn should be rejected");

        assert_eq!(error, NativeAgentSessionError::NonUserTurnMessage);
    }

    #[test]
    fn truncate_after_user_turns_keeps_provider_context_before_selected_turn() {
        let mut session = NativeAgentSession::new();
        session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("first question".to_string()),
            ))
            .expect("first turn should prepare");
        session.commit_pending_user();
        session.commit_turn_messages([Message::text(MessageRole::Assistant, "first answer")]);
        session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("second question".to_string()),
            ))
            .expect("second turn should prepare");
        session.commit_pending_user();
        session.commit_turn_messages([Message::text(MessageRole::Assistant, "second answer")]);

        session.truncate_after_user_turns(1);

        let visible_text = session
            .history()
            .iter()
            .map(Message::text_content)
            .collect::<Vec<_>>();
        assert_eq!(visible_text, vec!["first question", "first answer"]);
    }

    #[test]
    fn rollback_pending_user_discards_unstarted_turn() {
        let mut session = NativeAgentSession::new();
        session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("never sent".to_string()),
            ))
            .expect("turn should prepare");

        assert!(session.rollback_pending_user());
        assert!(session.history().is_empty());
    }

    #[test]
    fn commit_pending_user_persists_started_turn_once() {
        let mut session = NativeAgentSession::new();
        session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("sent".to_string()),
            ))
            .expect("turn should prepare");

        assert!(session.commit_pending_user());
        assert!(!session.commit_pending_user());
        assert_eq!(session.history()[0].text_content(), "sent");
        assert_eq!(session.history().len(), 1);
    }

    #[test]
    fn commit_turn_messages_keeps_tool_messages_for_future_turns() {
        let mut session = NativeAgentSession::new();
        session.commit_turn_messages([
            Message::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "call-1",
                    "read",
                    json!({ "path": "Cargo.toml" }),
                )],
            ),
            Message::tool_result(ToolResult::success("call-1", "read", "1\t[package]", None)),
            Message::text(MessageRole::Assistant, "done"),
        ]);

        let request = session
            .prepare_turn(&NativeAgentTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ChatMessage::user("next".to_string()),
            ))
            .expect("turn should prepare");

        assert_eq!(request.messages().len(), 4);
        assert_eq!(request.messages()[0].role, MessageRole::Assistant);
        assert!(matches!(
            &request.messages()[1].content[0],
            MessageContent::ToolResult(result) if result.name == "read"
        ));
    }

    #[test]
    fn chat_message_conversion_preserves_structured_blocks() {
        let message = message_from_chat_message(ChatMessage::user_with_blocks(
            "review image".to_string(),
            Some(vec![
                ChatMessageBlock::Text("review ".to_string()),
                ChatMessageBlock::Image {
                    data_base64: "iVBORw==".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                },
            ]),
        ));

        assert_eq!(message.role, MessageRole::User);
        assert!(matches!(
            &message.content[1],
            MessageContent::Image { mime_type, .. } if mime_type == "image/png"
        ));
    }
}
