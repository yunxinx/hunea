use std::{
    collections::HashSet,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
};

use provider_protocol::ConversationItem;
use runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState;
use runtime_domain::session::{
    MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN, MessageHistoryEntry, MessageHistoryRow,
    TranscriptReplayItem,
};
use tokio::task;

use crate::{
    ConfigSnapshot, ProjectDir, ResolveError, ResolvedSessionState, SessionBranchTreeSnapshot,
    SessionEntry, SessionEntryKind, SessionHeader, SessionId, SessionListOptions, SessionMeta,
    SessionStoreError, SessionTreeSnapshot, jsonl::JsonlLoader, meta_derive,
};

mod local;
mod memory;

#[cfg(test)]
mod tests;

pub use local::LocalSessionStore;
pub use memory::InMemorySessionStore;

/// `SessionLifecycleStore` 定义 session 创建、追加与恢复所需的持久化接口。
pub trait SessionLifecycleStore: Send + Sync {
    #[must_use]
    fn create_session<'a>(
        &'a self,
        header: SessionHeader,
    ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn append<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: ConversationItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn append_config_change<'a>(
        &'a self,
        session_id: &'a SessionId,
        snapshot: ConfigSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn append_transcript_replay<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: TranscriptReplayItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn set_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn resolve<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_session<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>>;
}

/// `SessionTreeStore` 定义 session tree 与 branch 视图所需的查询接口。
pub trait SessionTreeStore: Send + Sync {
    #[must_use]
    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_session_tree_for_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_session_branch_preview<'a>(
        &'a self,
        session_id: &'a SessionId,
        branch_row_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_session_branch_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<
        Box<dyn Future<Output = Result<SessionBranchTreeSnapshot, SessionStoreError>> + Send + 'a>,
    >;
}

/// `SessionCatalogStore` 定义 session 列表与元数据查询接口。
pub trait SessionCatalogStore: Send + Sync {
    #[must_use]
    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>>;
}

/// `SessionFlushStore` 定义持久化刷盘接口。
pub trait SessionFlushStore: Send + Sync {
    #[must_use]
    fn flush<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;
}

/// `MessageHistoryStore` 定义 message history 持久化接口。
pub trait MessageHistoryStore: Send + Sync {
    #[must_use]
    fn record_message_history<'a>(
        &'a self,
        text: &'a str,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_message_history_recent<'a>(
        &'a self,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    >;

    #[must_use]
    fn load_message_history_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MessageHistoryRow>, SessionStoreError>> + Send + 'a>>;

    /// 启动盲回溯缓存（固定 25 条，oldest-first）。
    fn load_message_history_startup_cache<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<MessageHistoryEntry>, SessionStoreError>> + Send + 'a>,
    > {
        self.load_message_history_recent(MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN)
    }
}

/// `PromptAssemblyStore` 定义全局 prompt assembly 持久化接口。
pub trait PromptAssemblyStore: Send + Sync {
    #[must_use]
    fn save_global_prompt_assembly_state<'a>(
        &'a self,
        state: &'a PromptAssemblyScopeState,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>>;

    #[must_use]
    fn load_global_prompt_assembly_state<'a>(
        &'a self,
    ) -> Pin<
        Box<dyn Future<Output = Result<PromptAssemblyScopeState, SessionStoreError>> + Send + 'a>,
    >;
}

/// `SessionStore` 聚合 runtime 当前需要的持久化能力。
pub trait SessionStore:
    SessionLifecycleStore
    + SessionTreeStore
    + SessionCatalogStore
    + SessionFlushStore
    + MessageHistoryStore
    + PromptAssemblyStore
{
}

pub(super) fn requested_leaf_id<'a>(
    entries: &'a [SessionEntry],
    leaf_id: Option<&'a str>,
) -> Result<&'a str, SessionStoreError> {
    if let Some(leaf_id) = leaf_id {
        return Ok(leaf_id);
    }

    entries
        .last()
        .map(|entry| entry.id.as_str())
        .ok_or_else(|| SessionStoreError::MissingHeader {
            message: "session is missing persisted entries".to_string(),
        })
}

pub(super) fn append_parent_id(entries: &[SessionEntry]) -> Option<String> {
    match entries.last().map(|entry| &entry.kind) {
        Some(SessionEntryKind::Leaf {
            target_id: Some(target_id),
        }) => Some(target_id.clone()),
        Some(SessionEntryKind::Leaf { target_id: None }) => latest_non_leaf_id(entries),
        Some(_) => entries.last().map(|entry| entry.id.clone()),
        None => None,
    }
}

pub(super) fn latest_non_leaf_id(entries: &[SessionEntry]) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|entry| !matches!(entry.kind, SessionEntryKind::Leaf { .. }))
        .map(|entry| entry.id.clone())
}

pub(super) fn update_append_projection(
    entry: &SessionEntry,
    next_parent_id: &mut Option<String>,
    latest_non_leaf_id: &mut Option<String>,
) {
    match &entry.kind {
        SessionEntryKind::Leaf {
            target_id: Some(target_id),
        } => {
            *next_parent_id = Some(target_id.clone());
        }
        SessionEntryKind::Leaf { target_id: None } => {
            *next_parent_id = latest_non_leaf_id.clone();
        }
        _ => {
            *latest_non_leaf_id = Some(entry.id.clone());
            *next_parent_id = Some(entry.id.clone());
        }
    }
}

pub(super) fn validate_append_kinds(kinds: &[SessionEntryKind]) -> Result<(), SessionStoreError> {
    for kind in kinds {
        if let SessionEntryKind::Item(item) = kind {
            item.validate()
                .map_err(|source| SessionStoreError::InvalidConversationItem { source })?;
        }
    }
    Ok(())
}

pub(super) fn entry_ids(entries: &[SessionEntry]) -> HashSet<String> {
    entries.iter().map(|entry| entry.id.clone()).collect()
}

pub(super) fn derive_store_session_meta(
    entries: &[SessionEntry],
    jsonl_path: PathBuf,
) -> Result<SessionMeta, SessionStoreError> {
    let size_bytes = std::fs::metadata(&jsonl_path)
        .ok()
        .map(|metadata| metadata.len());
    meta_derive::derive_session_meta(
        entries,
        jsonl_path,
        size_bytes,
        "session is missing header entry".to_string(),
    )
}

pub(super) fn current_timestamp_ms() -> Result<i64, SessionStoreError> {
    runtime_domain::time::unix_timestamp_ms().map_err(|_| SessionStoreError::CorruptIndex {
        message: "system clock is before Unix epoch or exceeds i64 millisecond range".to_string(),
    })
}

pub(super) fn resolve_error(error: ResolveError) -> SessionStoreError {
    match error {
        ResolveError::DuplicateId(id) => SessionStoreError::DuplicateId { id },
        ResolveError::DanglingParent(parent_id) => SessionStoreError::DanglingParent { parent_id },
        source => SessionStoreError::ResolveFailed { source },
    }
}

pub(super) async fn load_entries(path: &Path) -> Result<Vec<SessionEntry>, SessionStoreError> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || JsonlLoader::load(&path))
        .await
        .map_err(|_| SessionStoreError::MetadataTaskPanicked)?
}

pub(super) fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}
