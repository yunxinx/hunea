use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin};

use provider_protocol::ConversationItem;
use runtime_domain::prompt_assembly::persistence::{PromptAssemblyScope, PromptAssemblyScopeState};
use runtime_domain::session::{
    MessageHistoryEntry, MessageHistoryRow, TranscriptReplayItem, append_message_history_entry,
    should_record_message_history_text,
};
use tokio::sync::RwLock;

use crate::{
    ConfigSnapshot, ProjectDir, ResolveError, ResolvedSessionState, SessionBranchTreeSnapshot,
    SessionEntry, SessionEntryKind, SessionHeader, SessionId, SessionListOptions, SessionMeta,
    SessionStoreError, SessionTreeSnapshot, generate_entry_id, resolve as resolve_entries,
    resolve_state, session_branch_preview_snapshot, session_branch_tree_snapshot, session_filename,
    session_tree_snapshot, session_tree_snapshot_for_leaf,
};

use super::{
    MessageHistoryStore, PromptAssemblyStore, SessionCatalogStore, SessionFlushStore,
    SessionLifecycleStore, SessionStore, SessionTreeStore, append_parent_id, current_timestamp_ms,
    derive_store_session_meta, entry_ids, latest_non_leaf_id, requested_leaf_id, resolve_error,
    validate_append_kinds,
};

/// `InMemorySessionStore` 为运行时测试提供不落盘的 mock 实现。
///
/// 全局 message history 在内存中按与 `LocalSessionStore` 相同的相邻去重与条数上限语义维护，
/// 便于不依赖 SQLite 的集成测试。
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<SessionId, InMemorySession>>,
    message_history: RwLock<InMemoryMessageHistoryState>,
    global_prompt_assembly: RwLock<PromptAssemblyScopeState>,
}

struct InMemorySession {
    entries: Vec<SessionEntry>,
    jsonl_path: PathBuf,
}

#[derive(Default)]
struct InMemoryMessageHistoryState {
    next_row_id: i64,
    entries: Vec<InMemoryMessageHistoryEntry>,
}

#[derive(Clone)]
struct InMemoryMessageHistoryEntry {
    id: i64,
    ts: i64,
    text: String,
}

impl InMemorySessionStore {
    /// 创建空的内存 session store。
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            message_history: RwLock::new(InMemoryMessageHistoryState::default()),
            global_prompt_assembly: RwLock::new(PromptAssemblyScopeState::new(
                PromptAssemblyScope::Global,
            )),
        }
    }

    async fn record_message_history_entry(
        &self,
        text: String,
        limit: usize,
    ) -> Result<(), SessionStoreError> {
        if !should_record_message_history_text(&text) {
            return Ok(());
        }
        let ts = current_timestamp_ms()?;
        let mut history = self.message_history.write().await;
        history.apply_domain_append(MessageHistoryEntry { ts, text }, limit);
        Ok(())
    }

    async fn append_entry(
        &self,
        session_id: &SessionId,
        kind: SessionEntryKind,
    ) -> Result<String, SessionStoreError> {
        let mut entry_ids = self.append_entries(session_id, vec![kind]).await?;
        entry_ids
            .pop()
            .ok_or_else(|| SessionStoreError::CorruptIndex {
                message: "session append did not produce an entry id".to_string(),
            })
    }

    async fn append_entries(
        &self,
        session_id: &SessionId,
        kinds: Vec<SessionEntryKind>,
    ) -> Result<Vec<String>, SessionStoreError> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        validate_append_kinds(&kinds)?;

        let session_id = session_id.clone();
        let mut sessions = self.sessions.write().await;
        let session =
            sessions
                .get_mut(&session_id)
                .ok_or_else(|| SessionStoreError::SessionNotFound {
                    session_id: session_id.clone(),
                })?;
        let mut ids = entry_ids(&session.entries);
        let mut new_entry_ids = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let id = generate_entry_id(&ids);
            ids.insert(id.clone());
            let entry = SessionEntry {
                id: id.clone(),
                parent_id: append_parent_id(&session.entries),
                timestamp: current_timestamp_ms()?,
                kind,
            };
            session.entries.push(entry);
            new_entry_ids.push(id);
        }
        Ok(new_entry_ids)
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionLifecycleStore for InMemorySessionStore {
    fn create_session<'a>(
        &'a self,
        header: SessionHeader,
    ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = SessionId::new();
            let mut header = header;
            header.session_id = session_id.clone();
            let entry = SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: current_timestamp_ms()?,
                kind: SessionEntryKind::Header(header),
            };

            self.sessions.write().await.insert(
                session_id.clone(),
                InMemorySession {
                    entries: vec![entry],
                    jsonl_path: PathBuf::from(session_filename(&session_id)),
                },
            );

            Ok(session_id)
        })
    }

    fn append<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: ConversationItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::Item(item))
                .await
        })
    }

    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entries(
                session_id,
                items.into_iter().map(SessionEntryKind::Item).collect(),
            )
            .await
        })
    }

    fn append_config_change<'a>(
        &'a self,
        session_id: &'a SessionId,
        snapshot: ConfigSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::ConfigChange(snapshot))
                .await
                .map(|_| ())
        })
    }

    fn append_transcript_replay<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: TranscriptReplayItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::TranscriptReplay(item))
                .await
        })
    }

    fn set_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf_id = leaf_id.map(str::to_string);
            let mut sessions = self.sessions.write().await;
            let session = sessions.get_mut(&session_id).ok_or_else(|| {
                SessionStoreError::SessionNotFound {
                    session_id: session_id.clone(),
                }
            })?;
            if let Some(leaf_id) = requested_leaf_id.as_deref()
                && !session.entries.iter().any(|entry| entry.id == leaf_id)
            {
                return Err(SessionStoreError::ResolveFailed {
                    source: ResolveError::LeafNotFound(leaf_id.to_string()),
                });
            }

            let entry = SessionEntry {
                id: generate_entry_id(&entry_ids(&session.entries)),
                parent_id: latest_non_leaf_id(&session.entries),
                timestamp: current_timestamp_ms()?,
                kind: SessionEntryKind::Leaf {
                    target_id: requested_leaf_id,
                },
            };
            session.entries.push(entry);
            Ok(())
        })
    }

    fn resolve<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.map(str::to_string);
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            let requested_leaf_id = requested_leaf_id(&session.entries, requested_leaf.as_deref())?;
            resolve_entries(&session.entries, requested_leaf_id).map_err(resolve_error)
        })
    }

    fn load_session<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.map(str::to_string);
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            let requested_leaf_id = requested_leaf_id(&session.entries, requested_leaf.as_deref())?;
            resolve_state(&session.entries, requested_leaf_id).map_err(resolve_error)
        })
    }
}

impl SessionTreeStore for InMemorySessionStore {
    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            session_tree_snapshot(&session.entries).map_err(resolve_error)
        })
    }

    fn load_session_tree_for_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.to_string();
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            session_tree_snapshot_for_leaf(&session.entries, &requested_leaf).map_err(resolve_error)
        })
    }

    fn load_session_branch_preview<'a>(
        &'a self,
        session_id: &'a SessionId,
        branch_row_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_branch = branch_row_id.to_string();
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            session_branch_preview_snapshot(&session.entries, &requested_branch)
                .map_err(resolve_error)
        })
    }

    fn load_session_branch_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<
        Box<dyn Future<Output = Result<SessionBranchTreeSnapshot, SessionStoreError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let session_id = session_id.clone();
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            session_branch_tree_snapshot(&session.entries).map_err(resolve_error)
        })
    }
}

impl SessionCatalogStore for InMemorySessionStore {
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        _options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let sessions = self.sessions.read().await;
            let mut metas = sessions
                .values()
                .map(|session| {
                    derive_store_session_meta(&session.entries, session.jsonl_path.clone())
                })
                .collect::<Result<Vec<_>, _>>()?;
            metas.retain(|meta| meta.project_dir == *project_dir);
            metas.sort_by(|left, right| {
                right
                    .updated_at
                    .cmp(&left.updated_at)
                    .then_with(|| right.created_at.cmp(&left.created_at))
                    .then_with(|| right.session_id.cmp(&left.session_id))
            });
            Ok(metas)
        })
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            let sessions = self.sessions.read().await;
            let session =
                sessions
                    .get(&session_id)
                    .ok_or_else(|| SessionStoreError::SessionNotFound {
                        session_id: session_id.clone(),
                    })?;
            derive_store_session_meta(&session.entries, session.jsonl_path.clone())
        })
    }
}

impl SessionFlushStore for InMemorySessionStore {
    fn flush<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            let sessions = self.sessions.read().await;
            if sessions.contains_key(&session_id) {
                Ok(())
            } else {
                Err(SessionStoreError::SessionNotFound { session_id })
            }
        })
    }

    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async { Ok(()) })
    }
}

impl MessageHistoryStore for InMemorySessionStore {
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        let text = text.to_string();
        Box::pin(async move { self.record_message_history_entry(text, limit).await })
    }

    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let history = self.message_history.read().await;
            let start = history.entries.len().saturating_sub(limit);
            Ok(history.entries[start..]
                .iter()
                .map(|entry| MessageHistoryEntry {
                    ts: entry.ts,
                    text: entry.text.clone(),
                })
                .collect())
        })
    }

    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let history = self.message_history.read().await;
            Ok(history
                .iter()
                .map(|entry| MessageHistoryRow {
                    id: entry.id,
                    ts: entry.ts,
                    text: entry.text.clone(),
                })
                .collect())
        })
    }
}

impl PromptAssemblyStore for InMemorySessionStore {
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            if state.scope() != PromptAssemblyScope::Global {
                return Err(SessionStoreError::ConfigurationError {
                    message: format!(
                        "global prompt assembly persistence only accepts global scope, got {}",
                        state.scope().as_stored_value()
                    ),
                });
            }

            *self.global_prompt_assembly.write().await = state.clone();
            Ok(())
        })
    }

    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(self.global_prompt_assembly.read().await.clone()) })
    }
}

impl SessionStore for InMemorySessionStore {}

impl InMemoryMessageHistoryState {
    fn iter(&self) -> impl Iterator<Item = &InMemoryMessageHistoryEntry> {
        self.entries.iter()
    }

    fn apply_domain_append(&mut self, entry: MessageHistoryEntry, limit: usize) {
        let old_entries = std::mem::take(&mut self.entries);
        let mut logical: Vec<MessageHistoryEntry> = old_entries
            .iter()
            .map(|stored| MessageHistoryEntry {
                ts: stored.ts,
                text: stored.text.clone(),
            })
            .collect();
        append_message_history_entry(&mut logical, entry, limit);

        let mut old_by_key: std::collections::HashMap<(i64, String), i64> = old_entries
            .into_iter()
            .map(|stored| ((stored.ts, stored.text), stored.id))
            .collect();

        self.entries = logical
            .into_iter()
            .map(|entry| {
                let key = (entry.ts, entry.text.clone());
                let id = old_by_key
                    .remove(&key)
                    .unwrap_or_else(|| self.allocate_row_id());
                InMemoryMessageHistoryEntry {
                    id,
                    ts: entry.ts,
                    text: entry.text,
                }
            })
            .collect();
    }

    fn allocate_row_id(&mut self) -> i64 {
        self.next_row_id = self.next_row_id.saturating_add(1).max(1);
        self.next_row_id
    }
}
