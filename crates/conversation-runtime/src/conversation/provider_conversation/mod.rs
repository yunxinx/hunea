//! Provider-visible conversation assembly.

use std::sync::Arc;

use provider_protocol::{ConversationItem, Role};
use runtime_domain::{
    prompt_assembly::PromptPreludeSnapshot,
    session::{
        ConversationTurnRequest, RuntimeTarget, TranscriptReplayItem, TranscriptUserMessage,
    },
};
use session_store::{
    ConfigSnapshot, ResolvedSessionState, SessionHeader, SessionId, SessionStore, SessionStoreError,
};

use crate::{ProviderApiKey, ProviderKind};

mod history;
mod persistence;

pub use history::PersistedConversationItem;
pub(crate) use persistence::PreparedConversationPersistence;
use persistence::ProviderConversationPersistence;

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
    prompt_prelude: Option<PromptPreludeSnapshot>,
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
            .field("has_prompt_prelude", &self.prompt_prelude.is_some())
            .field("has_persistence", &self.persistence.is_some())
            .finish()
    }
}

impl PreparedConversationRequest {
    /// `from_turn` 根据当前 turn 与 provider-visible items 组装执行请求。
    pub(crate) fn from_turn(
        turn: &ConversationTurnRequest,
        items: Vec<ConversationItem>,
        prompt_prelude: Option<PromptPreludeSnapshot>,
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
            prompt_prelude,
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

    /// `prompt_prelude` 返回本次请求绑定的 prompt prelude 快照。
    pub fn prompt_prelude(&self) -> Option<&PromptPreludeSnapshot> {
        self.prompt_prelude.as_ref()
    }

    pub(crate) fn persistence_cloned(&self) -> Option<PreparedConversationPersistence> {
        self.persistence.clone()
    }
}

/// `ProviderConversation` 持有 provider-visible 内存对话。
#[derive(Default)]
pub struct ProviderConversation {
    system_prompt: Option<String>,
    prompt_prelude: Option<PromptPreludeSnapshot>,
    persisted_history: Vec<PersistedConversationItem>,
    pending_user_message: Option<ConversationItem>,
    persistence: Option<ProviderConversationPersistence>,
}

impl ProviderConversation {
    /// `new` 创建空的 provider-visible 对话。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `with_session_store` 创建带持久化能力的 provider-visible 对话。
    #[must_use = "creating a persisted provider conversation can fail and must be handled"]
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
    #[must_use = "restoring a persisted provider conversation can fail and must be handled"]
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
            system_prompt: restored_effective_system_prompt(restored_state),
            prompt_prelude: restored_state
                .latest_config
                .as_ref()
                .and_then(|config| config.prompt_prelude.clone()),
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
        self.system_prompt = None;
        self.prompt_prelude = None;
        self.persisted_history.clear();
        self.pending_user_message = None;
        if let Some(persistence) = self.persistence.as_mut() {
            persistence.session_id = None;
        }
    }

    /// `set_system_prompt` 设置会话级 system prompt。
    pub fn set_system_prompt(&mut self, prompt: Option<String>) {
        self.system_prompt = prompt.and_then(normalize_system_prompt);
        self.prompt_prelude = None;
    }

    /// `system_prompt` 返回当前生效的 system prompt。
    #[must_use]
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// `set_prompt_prelude` 设置当前 session 的已解析 prompt prelude。
    pub fn set_prompt_prelude(&mut self, prompt_prelude: Option<PromptPreludeSnapshot>) {
        self.system_prompt = prompt_prelude
            .as_ref()
            .and_then(PromptPreludeSnapshot::effective_system_prompt)
            .and_then(normalize_system_prompt);
        self.prompt_prelude = prompt_prelude;
    }

    /// `prompt_prelude` 返回当前 session 绑定的 prompt prelude 快照。
    #[must_use]
    pub fn prompt_prelude(&self) -> Option<&PromptPreludeSnapshot> {
        self.prompt_prelude.as_ref()
    }

    /// `context_budget_probe_items` 返回 `/context` 估算使用的只读会话快照。
    ///
    /// `/context` 会跨 tokio `spawn_blocking` 边界执行，同步借用最终仍需立刻转成 owned。
    /// 这里直接构造 `Arc<[ConversationItem]>`，把按需快照的成本表达清楚，避免借用 API
    /// 与后台输入所有权模型互相打架。
    #[must_use]
    pub fn context_budget_probe_items(&self) -> Arc<[ConversationItem]> {
        let mut items = Vec::with_capacity(
            self.persisted_history.len()
                + usize::from(self.system_prompt.is_some())
                + usize::from(self.pending_user_message.is_some()),
        );
        if let Some(system_prompt) = self.system_prompt.as_deref() {
            items.push(ConversationItem::text(Role::System, system_prompt));
        }
        items.extend(
            self.persisted_history
                .iter()
                .map(|persisted_item| persisted_item.item.clone()),
        );
        if let Some(pending) = self.pending_user_message.as_ref() {
            items.push(pending.clone());
        }
        Arc::from(items)
    }

    /// `session_id` 返回当前持久化 session id。
    #[must_use]
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
    #[must_use = "prepared turn requests must be submitted or explicitly discarded"]
    pub fn prepare_turn(
        &mut self,
        turn: &ConversationTurnRequest,
    ) -> Result<PreparedConversationRequest, ProviderConversationError> {
        self.prepare_turn_with_transcript(turn, None, Vec::new())
    }

    /// `prepare_turn_with_transcript` 允许调用方为当前 turn 指定单独的 transcript 展示消息。
    #[must_use = "prepared turn requests must be submitted or explicitly discarded"]
    pub fn prepare_turn_with_transcript(
        &mut self,
        turn: &ConversationTurnRequest,
        transcript_user_message: Option<TranscriptUserMessage>,
        transcript_replay_after_user: Vec<TranscriptReplayItem>,
    ) -> Result<PreparedConversationRequest, ProviderConversationError> {
        if turn.message().role() != Some(Role::User) {
            return Err(ProviderConversationError::NonUserTurnMessage);
        }
        if self.pending_user_message.is_some() {
            return Err(ProviderConversationError::PendingTurnAlreadyActive);
        }

        let user_message = turn.message().clone();
        self.pending_user_message = Some(user_message.clone());
        let transcript_user_message =
            transcript_user_message.unwrap_or_else(|| TranscriptUserMessage {
                content: user_message.text_content(),
                skill_bindings: Vec::new(),
                custom_prompt_bindings: Vec::new(),
            });
        let system_prompt = self.system_prompt.clone();
        let prompt_prelude = self.prompt_prelude.clone();
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
                        prompt_prelude,
                    },
                    current_user_message: user_message.clone(),
                    transcript_user_message,
                    transcript_replay_after_user,
                });

        Ok(PreparedConversationRequest::from_turn(
            turn,
            self.provider_items_with_pending_user(&user_message),
            self.prompt_prelude.clone(),
            persistence,
        ))
    }
}

fn normalize_system_prompt(prompt: String) -> Option<String> {
    let prompt = prompt.trim().to_string();
    (!prompt.is_empty()).then_some(prompt)
}

fn restored_effective_system_prompt(restored_state: &ResolvedSessionState) -> Option<String> {
    let config = restored_state.latest_config.as_ref()?;
    config
        .prompt_prelude
        .as_ref()
        .and_then(PromptPreludeSnapshot::effective_system_prompt)
        .or_else(|| config.system_prompt.clone())
        .and_then(normalize_system_prompt)
}

#[cfg(test)]
mod tests;
