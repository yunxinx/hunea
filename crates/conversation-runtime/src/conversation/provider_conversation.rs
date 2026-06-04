//! Provider-visible conversation assembly.

use provider_protocol::{ConversationItem, Role};
use runtime_domain::session::{ConversationTurnRequest, RuntimeTarget};

use crate::{ProviderApiKey, ProviderKind};

/// `ProviderConversationError` 描述 provider-visible 对话组装失败。
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProviderConversationError {
    #[error("Conversation turn request must carry a user message")]
    NonUserTurnMessage,
    #[error("Provider conversation already has a pending user turn")]
    PendingTurnAlreadyActive,
}

/// `PreparedConversationRequest` 是运行时实际执行时使用的完整请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedConversationRequest {
    provider_id: String,
    provider_kind: ProviderKind,
    model_id: String,
    base_url: Option<String>,
    api_key: Option<ProviderApiKey>,
    api_key_env: Option<String>,
    items: Vec<ConversationItem>,
}

impl PreparedConversationRequest {
    /// `new` 创建一次完整的对话执行请求。
    pub fn new(
        provider_id: impl Into<String>,
        provider_kind: ProviderKind,
        model_id: impl Into<String>,
        base_url: Option<String>,
        api_key: Option<ProviderApiKey>,
        api_key_env: Option<String>,
        items: Vec<ConversationItem>,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            provider_kind,
            model_id: model_id.into(),
            base_url,
            api_key,
            api_key_env,
            items,
        }
    }

    /// `target` 返回执行请求的统一 runtime 目标。
    pub fn target(&self) -> RuntimeTarget {
        RuntimeTarget::provider(self.provider_id.clone(), self.model_id.clone())
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

    /// `items` 返回 provider-visible 完整对话项。
    pub fn items(&self) -> &[ConversationItem] {
        &self.items
    }
}

/// `ProviderConversation` 持有 provider-visible 内存对话。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderConversation {
    system_prompt: Option<String>,
    history: Vec<ConversationItem>,
    pending_user_message: Option<ConversationItem>,
}

impl ProviderConversation {
    /// `new` 创建空的 provider-visible 对话。
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
        for (index, item) in self.history.iter().enumerate() {
            if item.role() != Some(Role::User) {
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
    pub fn history(&self) -> &[ConversationItem] {
        &self.history
    }

    /// `prepare_turn` 接受一个用户 turn，并构造完整执行请求。
    pub fn prepare_turn(
        &mut self,
        turn: &ConversationTurnRequest,
    ) -> Result<PreparedConversationRequest, ProviderConversationError> {
        if turn.message().role() != Some(Role::User) {
            return Err(ProviderConversationError::NonUserTurnMessage);
        }
        if self.pending_user_message.is_some() {
            return Err(ProviderConversationError::PendingTurnAlreadyActive);
        }

        let user_message = turn.message().clone();
        self.pending_user_message = Some(user_message.clone());

        Ok(PreparedConversationRequest::new(
            turn.provider_id().to_string(),
            turn.provider_kind(),
            turn.model_id().to_string(),
            turn.base_url().map(str::to_string),
            turn.api_key().cloned(),
            turn.api_key_env().map(str::to_string),
            self.provider_items_with_pending_user(&user_message),
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

    /// `commit_turn_items` 把 runtime 生成的 provider-visible 对话项写回会话历史。
    pub fn commit_turn_items(&mut self, items: impl IntoIterator<Item = ConversationItem>) {
        self.history.extend(items);
    }

    fn provider_items(&self) -> Vec<ConversationItem> {
        let mut items =
            Vec::with_capacity(self.history.len() + usize::from(self.system_prompt.is_some()));
        if let Some(system_prompt) = self.system_prompt.as_deref() {
            items.push(ConversationItem::text(Role::System, system_prompt));
        }
        items.extend(self.history.iter().cloned());
        items
    }

    fn provider_items_with_pending_user(
        &self,
        user_message: &ConversationItem,
    ) -> Vec<ConversationItem> {
        let mut items = self.provider_items();
        items.push(user_message.clone());
        items
    }
}

fn normalize_system_prompt(prompt: String) -> Option<String> {
    let prompt = prompt.trim().to_string();
    (!prompt.is_empty()).then_some(prompt)
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};

    use super::{ProviderConversation, ProviderConversationError};
    use crate::ProviderKind;
    use runtime_domain::session::ConversationTurnRequest;

    #[test]
    fn prepare_turn_uses_session_history_and_current_user_message() {
        let mut session = ProviderConversation::new();
        session.commit_turn_items([ConversationItem::text(Role::Assistant, "first answer")]);

        let request = session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "follow up"),
            ))
            .expect("turn should prepare");

        let visible_text = request
            .items()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>();
        assert_eq!(visible_text, vec!["first answer", "follow up"]);
        assert_eq!(session.history().len(), 1);
    }

    #[test]
    fn prepare_turn_prepends_system_prompt_without_persisting_it_in_history() {
        let mut session = ProviderConversation::new();
        session.set_system_prompt(Some("You are helpful".to_string()));

        let request = session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "hello"),
            ))
            .expect("turn should prepare");

        assert_eq!(request.items()[0].role(), Some(Role::System));
        assert_eq!(request.items()[0].text_content(), "You are helpful");
        assert!(session.history().is_empty());
    }

    #[test]
    fn prepare_turn_rejects_non_user_message() {
        let mut session = ProviderConversation::new();

        let error = session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::Assistant, "not a user turn"),
            ))
            .expect_err("assistant turn should be rejected");

        assert_eq!(error, ProviderConversationError::NonUserTurnMessage);
    }

    #[test]
    fn truncate_after_user_turns_keeps_provider_context_before_selected_turn() {
        let mut session = ProviderConversation::new();
        session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "first question"),
            ))
            .expect("first turn should prepare");
        session.commit_pending_user();
        session.commit_turn_items([ConversationItem::text(Role::Assistant, "first answer")]);
        session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "second question"),
            ))
            .expect("second turn should prepare");
        session.commit_pending_user();
        session.commit_turn_items([ConversationItem::text(Role::Assistant, "second answer")]);

        session.truncate_after_user_turns(1);

        let visible_text = session
            .history()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>();
        assert_eq!(visible_text, vec!["first question", "first answer"]);
    }

    #[test]
    fn rollback_pending_user_discards_unstarted_turn() {
        let mut session = ProviderConversation::new();
        session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "never sent"),
            ))
            .expect("turn should prepare");

        assert!(session.rollback_pending_user());
        assert!(session.history().is_empty());
    }

    #[test]
    fn commit_pending_user_persists_started_turn_once() {
        let mut session = ProviderConversation::new();
        session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "sent"),
            ))
            .expect("turn should prepare");

        assert!(session.commit_pending_user());
        assert!(!session.commit_pending_user());
        assert_eq!(session.history()[0].text_content(), "sent");
        assert_eq!(session.history().len(), 1);
    }

    #[test]
    fn commit_turn_items_keeps_tool_items_for_future_turns() {
        let mut session = ProviderConversation::new();
        session.commit_turn_items([
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "call-1",
                    "read",
                    r#"{"path":"Cargo.toml"}"#.to_string(),
                )],
            ),
            ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("1\t[package]".to_string())],
                false,
            ),
            ConversationItem::text(Role::Assistant, "done"),
        ]);

        let request = session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "qwen3",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "next"),
            ))
            .expect("turn should prepare");

        assert_eq!(request.items().len(), 4);
        assert_eq!(request.items()[0].role(), Some(Role::Assistant));
        assert!(matches!(
            &request.items()[1],
            ConversationItem::ToolResult {
                is_error: false,
                ..
            }
        ));
    }
}
