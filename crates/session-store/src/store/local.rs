use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard},
};

use tokio::{
    fs,
    sync::{Mutex, RwLock},
};

use crate::{
    ProjectDir, ResolveError, SessionEntry, SessionEntryKind, SessionId, SessionMeta,
    SessionStoreError, generate_entry_id, hunea_dir, metadata::MetadataIndex,
    recorder::SessionRecorder, session_filename,
};

use super::{
    append_parent_id, current_timestamp_ms, derive_store_session_meta, io_error,
    latest_non_leaf_id, load_entries, update_append_projection, validate_append_kinds,
};

mod commands;

pub(super) const MAX_OPEN_SESSION_HANDLES: usize = 64;

/// `LocalSessionStore` 使用 JSONL + SQLite 组合实现本地持久化。
pub struct LocalSessionStore {
    pub(super) hunea_dir: PathBuf,
    pub(super) recorders: RwLock<HashMap<SessionId, Arc<LocalSessionHandle>>>,
    pub(super) index: MetadataIndex,
}

pub(super) struct LocalSessionHandle {
    pub(super) jsonl_path: PathBuf,
    recorder: Arc<SessionRecorder>,
    pub(super) operation_lock: Mutex<()>,
    state: StdMutex<LocalSessionState>,
}

struct LocalSessionState {
    entries: Vec<SessionEntry>,
    entry_ids: HashSet<String>,
    pending_state_entries: Vec<SessionEntry>,
    session_meta: SessionMeta,
}

impl LocalSessionStore {
    /// 使用默认 hunea 配置目录打开本地 session store。
    #[must_use = "opening a local session store can fail and must be handled"]
    pub async fn open() -> Result<Self, SessionStoreError> {
        let hunea_dir = hunea_dir().ok_or_else(|| SessionStoreError::ConfigurationError {
            message: "failed to resolve hunea config directory".to_string(),
        })?;
        Self::open_in(hunea_dir).await
    }

    /// 使用显式目录打开本地 session store，便于测试与隔离环境。
    #[must_use = "opening a local session store can fail and must be handled"]
    pub async fn open_in(hunea_dir: PathBuf) -> Result<Self, SessionStoreError> {
        let sessions_dir = hunea_dir.join("sessions");
        fs::create_dir_all(&sessions_dir).await.map_err(io_error)?;

        let index_path = hunea_dir.join("index.sqlite");
        let should_backfill = !index_path.exists();
        let index = MetadataIndex::open(&index_path).await?;
        if should_backfill {
            index.backfill_from_jsonl(&sessions_dir).await?;
        }

        Ok(Self {
            hunea_dir,
            recorders: RwLock::new(HashMap::new()),
            index,
        })
    }

    /// 显式关闭所有已打开的 session recorder，并把 pending state 同步到索引。
    #[must_use = "shutdown flushes pending session data and its result must be handled"]
    pub async fn shutdown(self) -> Result<(), SessionStoreError> {
        let recorders = self.recorders.into_inner();
        for handle in recorders.into_values() {
            let meta = match Arc::try_unwrap(handle) {
                Ok(handle) => handle.shutdown().await?,
                Err(handle) => flush_handle(&handle).await?,
            };
            self.index.upsert_session(&meta).await?;
        }
        Ok(())
    }

    async fn handle_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<LocalSessionHandle>, SessionStoreError> {
        if let Some(handle) = self.recorders.read().await.get(session_id).cloned() {
            return Ok(handle);
        }

        let meta = self.index.get_session_meta(&session_id.to_string()).await?;
        let entries = load_entries(&meta.jsonl_path).await?;
        if entries.is_empty() {
            return Err(SessionStoreError::MissingHeader {
                message: format!("session `{session_id}` is missing persisted entries"),
            });
        }

        let mut recorders = self.recorders.write().await;
        if let Some(handle) = recorders.get(session_id).cloned() {
            return Ok(handle);
        }

        let handle = Arc::new(LocalSessionHandle::new(meta.jsonl_path, entries)?);
        recorders.insert(session_id.clone(), handle.clone());
        let evicted_handles = evict_idle_recorders(&mut recorders, session_id);
        drop(recorders);
        self.shutdown_evicted_recorders(evicted_handles).await?;
        Ok(handle)
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
        let handle = self.handle_for_session(&session_id).await?;
        let _guard = handle.operation_lock.lock().await;
        handle.recover_pending_state_entries().await?;

        let entries = {
            let state = handle.lock_state();
            if state.entries.is_empty() {
                return Err(SessionStoreError::SessionNotFound { session_id });
            }

            let mut batch_entry_ids = HashSet::with_capacity(kinds.len());
            let mut next_parent_id = append_parent_id(&state.entries);
            let mut latest_non_leaf = latest_non_leaf_id(&state.entries);
            let mut entries = Vec::with_capacity(kinds.len());
            for kind in kinds {
                let mut id = generate_entry_id(&state.entry_ids);
                while batch_entry_ids.contains(&id) {
                    id = generate_entry_id(&state.entry_ids);
                }
                batch_entry_ids.insert(id.clone());
                let entry = SessionEntry {
                    id: id.clone(),
                    parent_id: next_parent_id.clone(),
                    timestamp: current_timestamp_ms()?,
                    kind,
                };
                update_append_projection(&entry, &mut next_parent_id, &mut latest_non_leaf);
                entries.push(entry);
            }
            entries
        };

        let entry_ids = entries
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();
        handle.recorder.buffer_many(entries.clone())?;
        if let Err(error) = handle.recorder.persist().await {
            let mut state = handle.lock_state();
            for entry in entries {
                state.push_pending_state_entry(entry)?;
            }
            return Err(error);
        }

        let meta = {
            let mut state = handle.lock_state();
            for entry in entries {
                state.push_entry(entry, &handle.jsonl_path)?;
            }
            state.session_meta.clone()
        };

        self.index.upsert_session(&meta).await?;
        Ok(entry_ids)
    }

    async fn shutdown_evicted_recorders(
        &self,
        handles: Vec<Arc<LocalSessionHandle>>,
    ) -> Result<(), SessionStoreError> {
        for handle in handles {
            let meta = shutdown_handle(handle).await?;
            self.index.upsert_session(&meta).await?;
        }
        Ok(())
    }
}

async fn flush_handle(handle: &LocalSessionHandle) -> Result<SessionMeta, SessionStoreError> {
    let _guard = handle.operation_lock.lock().await;
    handle.flush_recorder_and_commit_pending().await?;
    let state = handle.lock_state();
    Ok(state.session_meta.clone())
}

async fn shutdown_handle(
    handle: Arc<LocalSessionHandle>,
) -> Result<SessionMeta, SessionStoreError> {
    match Arc::try_unwrap(handle) {
        Ok(handle) => handle.shutdown().await,
        Err(handle) => flush_handle(&handle).await,
    }
}

fn evict_idle_recorders(
    recorders: &mut HashMap<SessionId, Arc<LocalSessionHandle>>,
    keep_session_id: &SessionId,
) -> Vec<Arc<LocalSessionHandle>> {
    let overflow = recorders.len().saturating_sub(MAX_OPEN_SESSION_HANDLES);
    if overflow == 0 {
        return Vec::new();
    }

    let session_ids = recorders
        .iter()
        .filter(|(session_id, handle)| {
            *session_id != keep_session_id && Arc::strong_count(handle) == 1
        })
        .map(|(session_id, _)| session_id.clone())
        .take(overflow)
        .collect::<Vec<_>>();

    session_ids
        .into_iter()
        .filter_map(|session_id| recorders.remove(&session_id))
        .collect()
}

impl LocalSessionHandle {
    pub(super) fn new(
        jsonl_path: PathBuf,
        entries: Vec<SessionEntry>,
    ) -> Result<Self, SessionStoreError> {
        let session_meta = derive_store_session_meta(&entries, jsonl_path.clone())?;
        Ok(Self {
            recorder: Arc::new(SessionRecorder::new(jsonl_path.clone())?),
            jsonl_path,
            operation_lock: Mutex::new(()),
            state: StdMutex::new(LocalSessionState {
                entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
                entries,
                pending_state_entries: Vec::new(),
                session_meta,
            }),
        })
    }

    fn lock_state(&self) -> StdMutexGuard<'_, LocalSessionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn shutdown(self) -> Result<SessionMeta, SessionStoreError> {
        match Arc::try_unwrap(self.recorder) {
            Ok(recorder) => recorder.shutdown().await?,
            Err(recorder) => recorder.flush().await?,
        }
        let mut state = self
            .state
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.commit_pending_state_entries(&self.jsonl_path)?;
        Ok(state.session_meta.clone())
    }

    pub(super) async fn flush_recorder_and_commit_pending(&self) -> Result<(), SessionStoreError> {
        self.recorder.flush().await?;
        let mut state = self.lock_state();
        state.commit_pending_state_entries(&self.jsonl_path)
    }

    pub(super) async fn recover_pending_state_entries(&self) -> Result<(), SessionStoreError> {
        if !self.lock_state().has_pending_state_entries() {
            return Ok(());
        }

        // 上一次 persist 失败的 entries 仍由 recorder 按顺序保留；下一次 mutation
        // 必须先把它们落盘并合入内存视图，再计算新的 parent/id。
        self.recorder.flush().await?;
        let mut state = self.lock_state();
        state.commit_pending_state_entries(&self.jsonl_path)
    }
}

impl LocalSessionState {
    fn push_entry(
        &mut self,
        entry: SessionEntry,
        jsonl_path: &Path,
    ) -> Result<(), SessionStoreError> {
        if !self.entry_ids.insert(entry.id.clone()) {
            return Err(SessionStoreError::DuplicateId { id: entry.id });
        }

        self.entries.push(entry);
        self.session_meta = derive_store_session_meta(&self.entries, jsonl_path.to_path_buf())?;
        Ok(())
    }

    fn push_pending_state_entry(&mut self, entry: SessionEntry) -> Result<(), SessionStoreError> {
        if self.entry_ids.contains(&entry.id)
            || self
                .pending_state_entries
                .iter()
                .any(|pending| pending.id == entry.id)
        {
            return Err(SessionStoreError::DuplicateId { id: entry.id });
        }

        self.pending_state_entries.push(entry);
        Ok(())
    }

    fn has_pending_state_entries(&self) -> bool {
        !self.pending_state_entries.is_empty()
    }

    fn commit_pending_state_entries(&mut self, jsonl_path: &Path) -> Result<(), SessionStoreError> {
        if self.pending_state_entries.is_empty() {
            return Ok(());
        }

        let pending_entries = std::mem::take(&mut self.pending_state_entries);
        for entry in pending_entries {
            self.push_entry(entry, jsonl_path)?;
        }
        Ok(())
    }

    fn require_existing_entry(&self, leaf_id: &str) -> Result<(), SessionStoreError> {
        if self.entry_ids.contains(leaf_id) {
            Ok(())
        } else {
            Err(SessionStoreError::ResolveFailed {
                source: ResolveError::LeafNotFound(leaf_id.to_string()),
            })
        }
    }
}

pub(super) fn session_jsonl_path(
    hunea_dir: &Path,
    work_dir: &Path,
    session_id: &SessionId,
) -> PathBuf {
    hunea_dir
        .join("sessions")
        .join(ProjectDir::from_work_dir(work_dir).encoded_session_dir())
        .join(session_filename(session_id))
}
