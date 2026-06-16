//! Provider-visible conversation assembly.

use std::sync::Arc;

use provider_protocol::{ConversationItem, Role};
use runtime_domain::session::{ConversationTurnRequest, RuntimeTarget};
use session_store::{
    ConfigSnapshot, ResolvedSessionState, SessionHeader, SessionId, SessionStore, SessionStoreError,
};

use crate::{ProviderApiKey, ProviderKind};

/// `ProviderConversationError` 描述 provider-visible 对话组装失败。
#[derive(Debug, thiserror::Error)]
pub enum ProviderConversationError {
    #[error("Conversation turn request must carry a user message")]
    NonUserTurnMessage,
    #[error("Provider conversation already has a pending user turn")]
    PendingTurnAlreadyActive,
    #[error("Failed to access session store: {source}")]
    SessionStore {
        #[source]
        source: SessionStoreError,
    },
    #[error("Session persistence was initialized without an active session id")]
    MissingSessionId,
}

/// `PreparedConversationRequest` 是运行时实际执行时使用的完整请求。
pub struct PreparedConversationRequest {
    provider_id: String,
    provider_kind: ProviderKind,
    model_id: String,
    base_url: Option<String>,
    api_key: Option<ProviderApiKey>,
    api_key_env: Option<String>,
    items: Vec<ConversationItem>,
    persistence: Option<PreparedConversationPersistence>,
}

impl std::fmt::Debug for PreparedConversationRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedConversationRequest")
            .field("provider_id", &self.provider_id)
            .field("provider_kind", &self.provider_kind)
            .field("model_id", &self.model_id)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key)
            .field("api_key_env", &self.api_key_env)
            .field("items", &self.items)
            .field("has_persistence", &self.persistence.is_some())
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct PreparedConversationPersistence {
    pub(crate) store: Arc<dyn SessionStore>,
    pub(crate) session_id: Option<SessionId>,
    pub(crate) header_template: SessionHeader,
    pub(crate) config_snapshot: ConfigSnapshot,
    pub(crate) current_user_message: ConversationItem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedConversationItem {
    pub entry_id: Option<String>,
    pub item: ConversationItem,
}

impl PreparedConversationRequest {
    /// `from_turn` 根据当前 turn 与 provider-visible items 组装执行请求。
    pub(crate) fn from_turn(
        turn: &ConversationTurnRequest,
        items: Vec<ConversationItem>,
        persistence: Option<PreparedConversationPersistence>,
    ) -> Self {
        Self {
            provider_id: turn.provider_id().to_string(),
            provider_kind: turn.provider_kind(),
            model_id: turn.model_id().to_string(),
            base_url: turn.base_url().map(str::to_string),
            api_key: turn.api_key().cloned(),
            api_key_env: turn.api_key_env().map(str::to_string),
            items,
            persistence,
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

    pub(crate) fn persistence_cloned(&self) -> Option<PreparedConversationPersistence> {
        self.persistence.clone()
    }
}

/// `ProviderConversation` 持有 provider-visible 内存对话。
#[derive(Default)]
pub struct ProviderConversation {
    system_prompt: Option<String>,
    persisted_history: Vec<PersistedConversationItem>,
    pending_user_message: Option<ConversationItem>,
    persistence: Option<ProviderConversationPersistence>,
}

struct ProviderConversationPersistence {
    store: Arc<dyn SessionStore>,
    session_id: Option<SessionId>,
    header_template: SessionHeader,
}

impl ProviderConversation {
    /// `new` 创建空的 provider-visible 对话。
    pub fn new() -> Self {
        Self::default()
    }

    /// `with_session_store` 创建带持久化能力的 provider-visible 对话。
    pub fn with_session_store(
        store: Arc<dyn SessionStore>,
        header_template: SessionHeader,
    ) -> Result<Self, ProviderConversationError> {
        Ok(Self::from_resolved_session_store(
            store,
            header_template,
            None,
            &ResolvedSessionState::default(),
        ))
    }

    /// `with_resolved_session_store` 使用调用方已显式解析的 session state 构造对话。
    pub fn with_resolved_session_store(
        store: Arc<dyn SessionStore>,
        header_template: SessionHeader,
        session_id: Option<SessionId>,
        restored_state: &ResolvedSessionState,
    ) -> Result<Self, ProviderConversationError> {
        Ok(Self::from_resolved_session_store(
            store,
            header_template,
            session_id,
            restored_state,
        ))
    }

    fn from_resolved_session_store(
        store: Arc<dyn SessionStore>,
        header_template: SessionHeader,
        session_id: Option<SessionId>,
        restored_state: &ResolvedSessionState,
    ) -> Self {
        let persisted_history = restored_state
            .items
            .iter()
            .map(|entry| PersistedConversationItem {
                entry_id: Some(entry.entry_id.clone()),
                item: entry.item.clone(),
            })
            .collect::<Vec<_>>();

        Self {
            system_prompt: restored_state
                .latest_config
                .clone()
                .and_then(|config| config.system_prompt)
                .and_then(normalize_system_prompt),
            persisted_history,
            pending_user_message: None,
            persistence: Some(ProviderConversationPersistence {
                store,
                session_id,
                header_template,
            }),
        }
    }

    /// `clear` 清空当前会话。
    pub fn clear(&mut self) {
        self.persisted_history.clear();
        self.pending_user_message = None;
        if let Some(persistence) = self.persistence.as_mut() {
            persistence.session_id = None;
        }
    }

    /// `truncate_after_user_turns` 保留指定数量的已提交 user turns。
    pub fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, ProviderConversationError> {
        self.pending_user_message = None;
        let mut user_turn_count = 0usize;
        let mut truncate_index = self.persisted_history.len();
        for (index, item) in self.persisted_history.iter().enumerate() {
            if item.item.role() != Some(Role::User) {
                continue;
            }
            user_turn_count = user_turn_count.saturating_add(1);
            if user_turn_count > retained_user_turns {
                truncate_index = index;
                break;
            }
        }
        self.persisted_history.truncate(truncate_index);

        let leaf_update = if let Some(persistence) = self.persistence.as_ref()
            && let Some(session_id) = persistence.session_id.as_ref()
        {
            let leaf_id = self
                .persisted_history
                .last()
                .and_then(|item| item.entry_id.as_deref())
                .unwrap_or("header")
                .to_string();
            Some((session_id.clone(), leaf_id))
        } else {
            None
        };
        Ok(leaf_update)
    }

    /// `set_system_prompt` 设置会话级 system prompt。
    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt.and_then(normalize_system_prompt);
    }

    /// `system_prompt` 返回当前生效的 system prompt。
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// `history` 以零拷贝方式返回当前 provider-visible 历史。
    #[must_use]
    pub fn history(&self) -> impl ExactSizeIterator<Item = &ConversationItem> + '_ {
        self.persisted_history
            .iter()
            .map(|persisted_item| &persisted_item.item)
    }

    /// `is_history_empty` 返回当前 provider-visible 历史是否为空。
    #[must_use]
    pub fn is_history_empty(&self) -> bool {
        self.persisted_history.is_empty()
    }

    /// `history_len` 返回当前 provider-visible 历史项数量。
    #[must_use]
    pub fn history_len(&self) -> usize {
        self.persisted_history.len()
    }

    /// `session_id` 返回当前持久化 session id。
    pub fn session_id(&self) -> Option<&SessionId> {
        self.persistence
            .as_ref()
            .and_then(|persistence| persistence.session_id.as_ref())
    }

    /// `set_session_id` 记录 persistence actor 新建的 active session。
    pub fn set_session_id(&mut self, session_id: SessionId) {
        if let Some(persistence) = self.persistence.as_mut() {
            persistence.session_id = Some(session_id);
        }
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
        let system_prompt = self.system_prompt.clone();
        let persistence =
            self.persistence
                .as_ref()
                .map(|persistence| PreparedConversationPersistence {
                    store: persistence.store.clone(),
                    session_id: persistence.session_id.clone(),
                    header_template: persistence.header_template.clone(),
                    config_snapshot: ConfigSnapshot {
                        provider_id: turn.provider_id().to_string(),
                        model: turn.model_id().to_string(),
                        system_prompt,
                    },
                    current_user_message: user_message.clone(),
                });

        Ok(PreparedConversationRequest::from_turn(
            turn,
            self.provider_items_with_pending_user(&user_message),
            persistence,
        ))
    }

    /// `commit_pending_user` 把已开始发送给 provider 的当前用户消息写回会话历史。
    #[must_use]
    pub fn commit_pending_user(
        &mut self,
        entry_id: Option<String>,
        session_id: Option<SessionId>,
    ) -> bool {
        if let Some(session_id) = session_id {
            self.set_session_id(session_id);
        }
        let Some(user_message) = self.pending_user_message.take() else {
            return false;
        };
        self.persisted_history.push(PersistedConversationItem {
            entry_id,
            item: user_message,
        });
        true
    }

    /// `rollback_pending_user` 丢弃尚未开始发送给 provider 的当前用户消息。
    #[must_use]
    pub fn rollback_pending_user(&mut self) -> bool {
        self.pending_user_message.take().is_some()
    }

    /// `commit_turn_items` 把 runtime 生成的 provider-visible 对话项写回会话历史。
    pub fn commit_turn_items(
        &mut self,
        items: impl IntoIterator<Item = PersistedConversationItem>,
    ) {
        for item in items {
            self.persisted_history.push(item);
        }
    }

    /// `append_items` 追加 provider-visible 对话项。
    pub fn append_items(
        &mut self,
        items: Vec<ConversationItem>,
    ) -> Result<(), ProviderConversationError> {
        if items.is_empty() {
            return Ok(());
        }

        self.commit_turn_items(items.into_iter().map(|item| PersistedConversationItem {
            entry_id: None,
            item,
        }));
        Ok(())
    }

    fn provider_items(&self) -> Vec<ConversationItem> {
        let mut items = Vec::with_capacity(
            self.persisted_history.len() + usize::from(self.system_prompt.is_some()),
        );
        if let Some(system_prompt) = self.system_prompt.as_deref() {
            items.push(ConversationItem::text(Role::System, system_prompt));
        }
        items.extend(self.persisted_history.iter().map(|item| item.item.clone()));
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
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };

    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
    use session_store::{
        InMemorySessionStore, SessionHeader, SessionId, SessionStore, SessionStoreError,
    };

    use super::{PersistedConversationItem, ProviderConversation, ProviderConversationError};
    use crate::ProviderKind;
    use runtime_domain::session::ConversationTurnRequest;

    #[test]
    fn prepare_turn_uses_session_history_and_current_user_message() {
        let mut session = ProviderConversation::new();
        session.commit_turn_items([cached_item(ConversationItem::text(
            Role::Assistant,
            "first answer",
        ))]);

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
    fn history_exposes_borrowed_items_without_collecting_owned_history() {
        let mut session = ProviderConversation::new();
        session.commit_turn_items([cached_item(ConversationItem::text(Role::User, "hello"))]);

        let mut history = session.history();

        assert_eq!(history.len(), 1);
        assert_eq!(
            history.next().map(ConversationItem::text_content),
            Some("hello".to_string())
        );
        assert_eq!(history.len(), 0);
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
        assert!(session.is_history_empty());
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

        assert!(matches!(
            error,
            ProviderConversationError::NonUserTurnMessage
        ));
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
        assert!(session.commit_pending_user(None, None));
        session.commit_turn_items([cached_item(ConversationItem::text(
            Role::Assistant,
            "first answer",
        ))]);
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
        assert!(session.commit_pending_user(None, None));
        session.commit_turn_items([cached_item(ConversationItem::text(
            Role::Assistant,
            "second answer",
        ))]);

        session
            .truncate_after_user_turns(1)
            .expect("truncate should succeed");

        let visible_text = session
            .history()
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
        assert!(session.is_history_empty());
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

        assert!(session.commit_pending_user(None, None));
        assert!(!session.commit_pending_user(None, None));
        assert_eq!(
            session.history().next().map(ConversationItem::text_content),
            Some("sent".to_string())
        );
        assert_eq!(session.history().len(), 1);
    }

    #[test]
    fn commit_turn_items_keeps_tool_items_for_future_turns() {
        let mut session = ProviderConversation::new();
        session.commit_turn_items([
            cached_item(ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "call-1",
                    "read",
                    r#"{"path":"Cargo.toml"}"#.to_string(),
                )],
            )),
            cached_item(ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("1\t[package]".to_string())],
                false,
            )),
            cached_item(ConversationItem::text(Role::Assistant, "done")),
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

    #[test]
    fn persisted_conversation_loads_resolved_history_for_prepare_turn() {
        let work_dir = tempdir_path("resolved-history");
        let store = Arc::new(InMemorySessionStore::new());
        let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
            .expect("session should be created");
        let existing_items = vec![
            ConversationItem::text(Role::User, "hello"),
            ConversationItem::text(Role::Assistant, "hi"),
        ];
        for item in &existing_items {
            block_on_session(store.append(&session_id, item.clone()))
                .expect("history item should persist");
        }
        let restored_state = block_on_session(store.load_session(&session_id, None))
            .expect("session state should load");
        let store_trait: Arc<dyn SessionStore> = store;
        let mut session = ProviderConversation::with_resolved_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            Some(session_id),
            &restored_state,
        )
        .expect("persisted conversation should load history");

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
            .expect("turn should prepare from resolved history");

        let visible_text = request
            .items()
            .iter()
            .map(ConversationItem::text_content)
            .collect::<Vec<_>>();
        assert_eq!(visible_text, vec!["hello", "hi", "follow up"]);
        assert!(session.history().eq(existing_items.iter()));
    }

    #[test]
    fn append_items_keeps_provider_history_in_sync() {
        let work_dir = tempdir_path("append-items");
        let store_trait: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
        let mut session = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
        )
        .expect("persisted conversation should initialize");
        let items = vec![
            ConversationItem::text(Role::User, "question"),
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new("call-1", "read", "{}")],
            ),
            ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("done".to_string())],
                false,
            ),
        ];

        session
            .append_items(items.clone())
            .expect("items should append to provider history");

        assert!(session.history().eq(items.iter()));
    }

    #[test]
    fn persisted_conversation_restores_latest_system_prompt_snapshot() {
        let work_dir = tempdir_path("restored-config");
        let store = Arc::new(InMemorySessionStore::new());
        let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
            .expect("session should be created");
        block_on_session(store.append(&session_id, ConversationItem::text(Role::User, "hello")))
            .expect("user item should persist");
        block_on_session(store.append_config_change(
            &session_id,
            session_store::ConfigSnapshot {
                provider_id: "local".to_string(),
                model: "qwen3".to_string(),
                system_prompt: Some("keep it sharp".to_string()),
            },
        ))
        .expect("config snapshot should persist");

        let restored_state = block_on_session(store.load_session(&session_id, None))
            .expect("session state should load");
        let store_trait: Arc<dyn SessionStore> = store;
        let session = ProviderConversation::with_resolved_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            Some(session_id),
            &restored_state,
        )
        .expect("persisted conversation should load config");

        assert_eq!(session.system_prompt(), Some("keep it sharp"));
    }

    #[test]
    fn truncate_after_user_turns_branches_within_existing_session() {
        let work_dir = tempdir_path("truncate-branch");
        let store = Arc::new(InMemorySessionStore::new());
        let initial_history = vec![
            ConversationItem::text(Role::User, "first"),
            ConversationItem::text(Role::Assistant, "alpha"),
            ConversationItem::text(Role::User, "second"),
            ConversationItem::text(Role::Assistant, "beta"),
        ];
        let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
            .expect("session should be created");
        for item in &initial_history {
            block_on_session(store.append(&session_id, item.clone()))
                .expect("history item should persist");
        }
        let restored_state = block_on_session(store.load_session(&session_id, None))
            .expect("session state should load");
        let first_assistant_entry_id = restored_state.items[1].entry_id.clone();
        let store_trait: Arc<dyn SessionStore> = store;
        let mut session = ProviderConversation::with_resolved_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            Some(session_id.clone()),
            &restored_state,
        )
        .expect("persisted conversation should initialize");

        let leaf_update = session
            .truncate_after_user_turns(1)
            .expect("truncate should keep original session");

        assert_eq!(
            leaf_update,
            Some((session_id, first_assistant_entry_id)),
            "truncate should report the leaf update for async persistence"
        );
        let expected_history = [
            ConversationItem::text(Role::User, "first"),
            ConversationItem::text(Role::Assistant, "alpha"),
        ];
        assert!(session.history().eq(expected_history.iter()));
    }

    fn sample_header(work_dir: &Path, model: &str) -> SessionHeader {
        SessionHeader {
            session_id: SessionId::new(),
            work_dir: work_dir.to_path_buf(),
            session_name: Some("test-session".to_string()),
            initial_model: model.to_string(),
            git_head: Some("abc123".to_string()),
            cli_version: Some("0.5.7".to_string()),
        }
    }

    fn tempdir_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lumos-provider-conversation-{label}-{}",
            std::process::id()
        ))
    }

    fn cached_item(item: ConversationItem) -> PersistedConversationItem {
        PersistedConversationItem {
            entry_id: None,
            item,
        }
    }

    fn block_on_session<T>(
        future: impl std::future::Future<Output = Result<T, SessionStoreError>>,
    ) -> Result<T, SessionStoreError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        runtime.block_on(future)
    }
}
