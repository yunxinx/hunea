//! Provider-visible conversation assembly.

use std::{future::Future, sync::Arc};

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
    #[error("Failed to start blocking session runtime: {message}")]
    SessionRuntime { message: String },
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
    pub(crate) session_id: SessionId,
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
    history: Vec<ConversationItem>,
    persisted_history: Vec<PersistedConversationItem>,
    pending_user_message: Option<ConversationItem>,
    persistence: Option<ProviderConversationPersistence>,
}

struct ProviderConversationPersistence {
    bridge: SessionStoreBridge,
    store: Arc<dyn SessionStore>,
    session_id: Option<SessionId>,
    header_template: SessionHeader,
}

#[derive(Clone)]
struct SessionStoreBridge {
    runtime: Arc<tokio::runtime::Runtime>,
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
        session_id: Option<SessionId>,
    ) -> Result<Self, ProviderConversationError> {
        let bridge = SessionStoreBridge::new()?;
        let restored_state = if let Some(session_id) = session_id.as_ref() {
            bridge.block_on(store.load_session(session_id, None))?
        } else {
            ResolvedSessionState::default()
        };
        let history = restored_state
            .items
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let persisted_history = restored_state
            .items
            .into_iter()
            .map(|entry| PersistedConversationItem {
                entry_id: Some(entry.entry_id),
                item: entry.item,
            })
            .collect::<Vec<_>>();

        Ok(Self {
            system_prompt: restored_state
                .latest_config
                .and_then(|config| config.system_prompt)
                .and_then(normalize_system_prompt),
            history,
            persisted_history,
            pending_user_message: None,
            persistence: Some(ProviderConversationPersistence {
                bridge,
                store,
                session_id,
                header_template,
            }),
        })
    }

    /// `clear` 清空当前会话。
    pub fn clear(&mut self) {
        self.history.clear();
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
    ) -> Result<(), ProviderConversationError> {
        self.pending_user_message = None;
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
        self.persisted_history.truncate(truncate_index);

        if let Some(persistence) = self.persistence.as_mut()
            && let Some(session_id) = persistence.session_id.as_ref()
        {
            let leaf_id = self
                .persisted_history
                .last()
                .and_then(|item| item.entry_id.as_deref())
                .unwrap_or("header");
            persistence
                .bridge
                .block_on(persistence.store.set_leaf(session_id, Some(leaf_id)))?;
        }
        Ok(())
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

    /// `session_id` 返回当前持久化 session id。
    pub fn session_id(&self) -> Option<&SessionId> {
        self.persistence
            .as_ref()
            .and_then(|persistence| persistence.session_id.as_ref())
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
        let persistence = self
            .ensure_persistence(turn.model_id())?
            .map(|persistence| PreparedConversationPersistence {
                store: persistence.store.clone(),
                session_id: persistence
                    .session_id
                    .clone()
                    .expect("session should exist after ensure_persistence"),
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
    pub fn commit_pending_user(&mut self, entry_id: Option<String>) -> bool {
        let Some(user_message) = self.pending_user_message.take() else {
            return false;
        };
        self.history.push(user_message.clone());
        self.persisted_history.push(PersistedConversationItem {
            entry_id,
            item: user_message,
        });
        true
    }

    /// `rollback_pending_user` 丢弃尚未开始发送给 provider 的当前用户消息。
    pub fn rollback_pending_user(&mut self) -> bool {
        self.pending_user_message.take().is_some()
    }

    /// `commit_turn_items` 把 runtime 生成的 provider-visible 对话项写回会话历史。
    pub fn commit_turn_items(
        &mut self,
        items: impl IntoIterator<Item = PersistedConversationItem>,
    ) {
        for item in items {
            self.history.push(item.item.clone());
            self.persisted_history.push(item);
        }
    }

    /// `append_items` 同步持久化并追加 provider-visible 对话项。
    pub fn append_items(
        &mut self,
        items: Vec<ConversationItem>,
    ) -> Result<(), ProviderConversationError> {
        if items.is_empty() {
            return Ok(());
        }

        if let Some(persistence) = self.ensure_persistence_with_template_model()? {
            let mut persisted_items = Vec::with_capacity(items.len());
            for item in items {
                let session_id = persistence
                    .session_id
                    .as_ref()
                    .expect("session should exist after ensure_persistence");
                let entry_id = persistence
                    .bridge
                    .block_on(persistence.store.append(session_id, item.clone()))?;
                persisted_items.push(PersistedConversationItem {
                    entry_id: Some(entry_id),
                    item,
                });
            }
            self.commit_turn_items(persisted_items);
            return Ok(());
        }

        self.commit_turn_items(items.into_iter().map(|item| PersistedConversationItem {
            entry_id: None,
            item,
        }));
        Ok(())
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

    fn ensure_persistence(
        &mut self,
        model_id: &str,
    ) -> Result<Option<&ProviderConversationPersistence>, ProviderConversationError> {
        let Some(persistence) = self.persistence.as_mut() else {
            return Ok(None);
        };
        if persistence.session_id.is_none() {
            let mut header = persistence.header_template.clone();
            header.initial_model = model_id.to_string();
            let session_id = persistence
                .bridge
                .block_on(persistence.store.create_session(header))?;
            persistence.session_id = Some(session_id.clone());
            if !self.persisted_history.is_empty() {
                let mut replayed_history = Vec::with_capacity(self.persisted_history.len());
                for item in &self.persisted_history {
                    let entry_id = persistence
                        .bridge
                        .block_on(persistence.store.append(&session_id, item.item.clone()))?;
                    replayed_history.push(PersistedConversationItem {
                        entry_id: Some(entry_id),
                        item: item.item.clone(),
                    });
                }
                self.persisted_history = replayed_history;
            }
        }
        Ok(Some(persistence))
    }

    fn ensure_persistence_with_template_model(
        &mut self,
    ) -> Result<Option<&ProviderConversationPersistence>, ProviderConversationError> {
        let model_id = self
            .persistence
            .as_ref()
            .map(|persistence| persistence.header_template.initial_model.clone())
            .unwrap_or_default();
        self.ensure_persistence(&model_id)
    }
}

fn normalize_system_prompt(prompt: String) -> Option<String> {
    let prompt = prompt.trim().to_string();
    (!prompt.is_empty()).then_some(prompt)
}

#[cfg(test)]
fn block_on_session<T>(
    future: impl Future<Output = Result<T, SessionStoreError>>,
) -> Result<T, ProviderConversationError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| ProviderConversationError::SessionRuntime {
            message: error.to_string(),
        })?;
    runtime
        .block_on(future)
        .map_err(|source| ProviderConversationError::SessionStore { source })
}

impl SessionStoreBridge {
    fn new() -> Result<Self, ProviderConversationError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ProviderConversationError::SessionRuntime {
                message: error.to_string(),
            })?;
        Ok(Self {
            runtime: Arc::new(runtime),
        })
    }

    fn block_on<T>(
        &self,
        future: impl Future<Output = Result<T, SessionStoreError>>,
    ) -> Result<T, ProviderConversationError> {
        self.runtime
            .block_on(future)
            .map_err(|source| ProviderConversationError::SessionStore { source })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        path::{Path, PathBuf},
        pin::Pin,
        sync::{Arc, Mutex},
    };

    use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
    use session_store::{
        InMemorySessionStore, SessionHeader, SessionId, SessionMeta, SessionStore,
        SessionStoreError,
    };

    use super::{
        PersistedConversationItem, ProviderConversation, ProviderConversationError,
        block_on_session,
    };
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
        session.commit_pending_user(None);
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
        session.commit_pending_user(None);
        session.commit_turn_items([cached_item(ConversationItem::text(
            Role::Assistant,
            "second answer",
        ))]);

        session
            .truncate_after_user_turns(1)
            .expect("truncate should succeed");

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

        assert!(session.commit_pending_user(None));
        assert!(!session.commit_pending_user(None));
        assert_eq!(session.history()[0].text_content(), "sent");
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
        let store_trait: Arc<dyn SessionStore> = store;
        let mut session = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            Some(session_id),
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
        assert_eq!(session.history(), existing_items.as_slice());
    }

    #[test]
    fn append_items_keeps_history_and_resolve_in_sync() {
        let work_dir = tempdir_path("append-items");
        let store = Arc::new(InMemorySessionStore::new());
        let store_trait: Arc<dyn SessionStore> = store.clone();
        let mut session = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            None,
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
            .expect("items should append to store");

        let metas = block_on_session(store.list_sessions(work_dir.to_string_lossy().as_ref()))
            .expect("session meta should list");
        assert_eq!(metas.len(), 1);
        let resolved = block_on_session(store.resolve(&metas[0].session_id, None))
            .expect("session should resolve appended items");

        assert_eq!(resolved, items);
        assert_eq!(session.history(), items.as_slice());
    }

    #[test]
    fn append_items_error_does_not_pollute_cached_history() {
        let work_dir = tempdir_path("append-failure");
        let store: Arc<dyn SessionStore> = Arc::new(FailingAppendStore::new());
        let mut session = ProviderConversation::with_session_store(
            store,
            sample_header(&work_dir, "qwen3"),
            None,
        )
        .expect("persisted conversation should initialize");

        let error = session
            .append_items(vec![ConversationItem::text(Role::Assistant, "fail")])
            .expect_err("append should surface store error");

        assert!(matches!(
            error,
            ProviderConversationError::SessionStore { .. }
        ));
        assert!(session.history().is_empty());
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

        let store_trait: Arc<dyn SessionStore> = store;
        let session = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            Some(session_id),
        )
        .expect("persisted conversation should load config");

        assert_eq!(session.system_prompt(), Some("keep it sharp"));
    }

    #[test]
    fn truncate_after_user_turns_branches_within_existing_session() {
        let work_dir = tempdir_path("truncate-branch");
        let store = Arc::new(InMemorySessionStore::new());
        let store_trait: Arc<dyn SessionStore> = store.clone();
        let mut session = ProviderConversation::with_session_store(
            store_trait,
            sample_header(&work_dir, "qwen3"),
            None,
        )
        .expect("persisted conversation should initialize");
        let initial_history = vec![
            ConversationItem::text(Role::User, "first"),
            ConversationItem::text(Role::Assistant, "alpha"),
            ConversationItem::text(Role::User, "second"),
            ConversationItem::text(Role::Assistant, "beta"),
        ];
        session
            .append_items(initial_history)
            .expect("initial history should persist");

        session
            .truncate_after_user_turns(1)
            .expect("truncate should keep original session");
        session
            .append_items(vec![ConversationItem::text(Role::Assistant, "branched")])
            .expect("branched item should persist");

        let metas = block_on_session(store.list_sessions(work_dir.to_string_lossy().as_ref()))
            .expect("session meta should list");
        assert_eq!(metas.len(), 1, "truncate should keep a single session");

        let resolved = block_on_session(store.resolve(&metas[0].session_id, None))
            .expect("session should resolve branched items");
        assert_eq!(
            resolved,
            vec![
                ConversationItem::text(Role::User, "first"),
                ConversationItem::text(Role::Assistant, "alpha"),
                ConversationItem::text(Role::Assistant, "branched"),
            ]
        );
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

    struct FailingAppendStore {
        session_id: Mutex<Option<SessionId>>,
    }

    impl FailingAppendStore {
        fn new() -> Self {
            Self {
                session_id: Mutex::new(None),
            }
        }
    }

    impl SessionStore for FailingAppendStore {
        fn create_session<'a>(
            &'a self,
            _header: SessionHeader,
        ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async move {
                let session_id = SessionId::new();
                *self
                    .session_id
                    .lock()
                    .expect("session id lock should not poison") = Some(session_id.clone());
                Ok(session_id)
            })
        }

        fn append<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _item: ConversationItem,
        ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
            Box::pin(async move {
                Err(SessionStoreError::IoError {
                    source: std::io::Error::other("append failed"),
                })
            })
        }

        fn append_config_change<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _snapshot: session_store::ConfigSnapshot,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn append_transcript_replay<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _item: runtime_domain::session::TranscriptReplayItem,
        ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
            Box::pin(async { Ok("replay-1".to_string()) })
        }

        fn set_leaf<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _leaf_id: Option<&'a str>,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn resolve<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn load_session<'a>(
            &'a self,
            _session_id: &'a SessionId,
            _leaf_id: Option<&'a str>,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<session_store::ResolvedSessionState, SessionStoreError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(session_store::ResolvedSessionState::default()) })
        }

        fn load_session_tree<'a>(
            &'a self,
            _session_id: &'a SessionId,
        ) -> Pin<
            Box<
                dyn Future<Output = Result<session_store::SessionTreeSnapshot, SessionStoreError>>
                    + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(session_store::SessionTreeSnapshot::default()) })
        }

        fn list_sessions<'a>(
            &'a self,
            _project_dir: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
        {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn get_session_meta<'a>(
            &'a self,
            session_id: &'a SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>>
        {
            let session_id = session_id.clone();
            Box::pin(async move {
                Ok(SessionMeta {
                    session_id,
                    project_dir: String::new(),
                    title: String::new(),
                    preview: None,
                    first_user_preview: None,
                    last_assistant_preview: None,
                    total_tokens: 0,
                    model: None,
                    created_at: 0,
                    updated_at: 0,
                    git_head: None,
                    work_dir: PathBuf::new(),
                    jsonl_path: PathBuf::new(),
                })
            })
        }

        fn flush<'a>(
            &'a self,
            _session_id: &'a SessionId,
        ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }
}
