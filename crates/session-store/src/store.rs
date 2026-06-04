use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use provider_protocol::{ConversationItem, Role};
use tokio::{
    fs,
    sync::{Mutex, RwLock},
    task,
};

use crate::{
    ResolveError, SessionEntry, SessionEntryKind, SessionHeader, SessionId, SessionMeta,
    SessionStoreError, encode_project_dir, generate_entry_id, hunea_dir, jsonl::JsonlLoader,
    metadata::MetadataIndex, recorder::SessionRecorder, resolve as resolve_entries,
    session_filename,
};

/// `SessionStore` 定义 conversation-runtime 依赖的持久化接口。
pub trait SessionStore: Send + Sync {
    fn create_session(
        &self,
        header: SessionHeader,
    ) -> impl Future<Output = Result<SessionId, SessionStoreError>> + Send;

    fn append(
        &self,
        session_id: &SessionId,
        item: ConversationItem,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send;

    fn set_leaf(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send;

    fn resolve(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> impl Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send;

    fn list_sessions(
        &self,
        project_dir: &str,
    ) -> impl Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send;

    fn get_session_meta(
        &self,
        session_id: &SessionId,
    ) -> impl Future<Output = Result<SessionMeta, SessionStoreError>> + Send;

    fn flush(
        &self,
        session_id: &SessionId,
    ) -> impl Future<Output = Result<(), SessionStoreError>> + Send;
}

/// `LocalSessionStore` 使用 JSONL + SQLite 组合实现本地持久化。
pub struct LocalSessionStore {
    hunea_dir: PathBuf,
    recorders: RwLock<HashMap<SessionId, Arc<LocalSessionHandle>>>,
    index: MetadataIndex,
}

struct LocalSessionHandle {
    jsonl_path: PathBuf,
    recorder: Arc<SessionRecorder>,
    operation_lock: Mutex<()>,
    state: StdMutex<LocalSessionState>,
}

struct LocalSessionState {
    entries: Vec<SessionEntry>,
    entry_ids: HashSet<String>,
    session_meta: SessionMeta,
}

impl LocalSessionStore {
    /// 使用默认 hunea 配置目录打开本地 session store。
    pub async fn open() -> Result<Self, SessionStoreError> {
        let hunea_dir = hunea_dir().ok_or_else(|| SessionStoreError::IndexInconsistent {
            message: "failed to resolve hunea config directory".to_string(),
        })?;
        Self::open_in(hunea_dir).await
    }

    /// 使用显式目录打开本地 session store，便于测试与隔离环境。
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
            return Err(SessionStoreError::IndexInconsistent {
                message: format!("session `{session_id}` is missing persisted entries"),
            });
        }

        let handle = Arc::new(LocalSessionHandle::new(meta.jsonl_path, entries)?);
        let mut recorders = self.recorders.write().await;
        Ok(recorders
            .entry(session_id.clone())
            .or_insert_with(|| handle.clone())
            .clone())
    }
}

impl SessionStore for LocalSessionStore {
    async fn create_session(&self, header: SessionHeader) -> Result<SessionId, SessionStoreError> {
        let session_id = SessionId::new();
        let mut header = header;
        header.session_id = session_id.clone();

        let jsonl_path = session_jsonl_path(&self.hunea_dir, &header.work_dir, &session_id);
        let header_entry = SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: current_timestamp_ms()?,
            kind: SessionEntryKind::Header(header),
        };
        let handle = Arc::new(LocalSessionHandle::new(
            jsonl_path.clone(),
            vec![header_entry.clone()],
        )?);

        {
            let _guard = handle.operation_lock.lock().await;
            handle.recorder.buffer(header_entry)?;
            handle.recorder.persist().await?;
        }

        let meta = handle.lock_state().session_meta.clone();
        self.recorders
            .write()
            .await
            .insert(session_id.clone(), handle);
        self.index.upsert_session(&meta).await?;

        Ok(session_id)
    }

    async fn append(
        &self,
        session_id: &SessionId,
        item: ConversationItem,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let handle = self.handle_for_session(&session_id).await?;
        let _guard = handle.operation_lock.lock().await;

        let entry = {
            let state = handle.lock_state();
            if state.entries.is_empty() {
                return Err(SessionStoreError::SessionNotFound { session_id });
            }

            SessionEntry {
                id: generate_entry_id(&state.entry_ids),
                parent_id: append_parent_id(&state.entries),
                timestamp: current_timestamp_ms()?,
                kind: SessionEntryKind::Item(item),
            }
        };

        handle.recorder.buffer(entry.clone())?;
        handle.recorder.persist().await?;

        let meta = {
            let mut state = handle.lock_state();
            state.push_entry(entry, &handle.jsonl_path)?;
            state.session_meta.clone()
        };

        self.index.upsert_session(&meta).await
    }

    async fn set_leaf(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let requested_leaf_id = leaf_id.map(str::to_string);
        let handle = self.handle_for_session(&session_id).await?;
        let _guard = handle.operation_lock.lock().await;

        let entry = {
            let state = handle.lock_state();
            if state.entries.is_empty() {
                return Err(SessionStoreError::SessionNotFound { session_id });
            }

            if let Some(leaf_id) = requested_leaf_id.as_deref() {
                state.require_existing_entry(leaf_id)?;
            }

            SessionEntry {
                id: generate_entry_id(&state.entry_ids),
                parent_id: latest_non_leaf_id(&state.entries),
                timestamp: current_timestamp_ms()?,
                kind: SessionEntryKind::Leaf {
                    target_id: requested_leaf_id.clone(),
                },
            }
        };

        handle.recorder.buffer(entry.clone())?;
        handle.recorder.persist().await?;

        let meta = {
            let mut state = handle.lock_state();
            state.push_entry(entry, &handle.jsonl_path)?;
            state.session_meta.clone()
        };

        self.index.upsert_session(&meta).await
    }

    async fn resolve(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> Result<Vec<ConversationItem>, SessionStoreError> {
        let session_id = session_id.clone();
        let requested_leaf = leaf_id.map(str::to_string);
        let handle = self.handle_for_session(&session_id).await?;
        let _guard = handle.operation_lock.lock().await;
        let state = handle.lock_state();
        let requested_leaf_id =
            requested_leaf_id(state.entries.as_slice(), requested_leaf.as_deref())?;
        resolve_entries(&state.entries, requested_leaf_id).map_err(resolve_error)
    }

    async fn list_sessions(
        &self,
        project_dir: &str,
    ) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let project_dir = normalize_project_dir(Path::new(project_dir));
        self.index.list_sessions(&project_dir).await
    }

    async fn get_session_meta(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionMeta, SessionStoreError> {
        let session_id = session_id.clone();
        if let Some(handle) = self.recorders.read().await.get(&session_id).cloned() {
            let _guard = handle.operation_lock.lock().await;
            return Ok(handle.lock_state().session_meta.clone());
        }

        self.index.get_session_meta(&session_id.to_string()).await
    }

    async fn flush(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let handle = self.handle_for_session(&session_id).await?;
        let _guard = handle.operation_lock.lock().await;
        handle.recorder.flush().await?;
        let meta = handle.lock_state().session_meta.clone();
        self.index.upsert_session(&meta).await
    }
}

impl LocalSessionHandle {
    fn new(jsonl_path: PathBuf, entries: Vec<SessionEntry>) -> Result<Self, SessionStoreError> {
        let session_meta = derive_session_meta(&entries, jsonl_path.clone())?;
        Ok(Self {
            recorder: Arc::new(SessionRecorder::new(jsonl_path.clone())),
            jsonl_path,
            operation_lock: Mutex::new(()),
            state: StdMutex::new(LocalSessionState {
                entry_ids: entries.iter().map(|entry| entry.id.clone()).collect(),
                entries,
                session_meta,
            }),
        })
    }

    fn lock_state(&self) -> StdMutexGuard<'_, LocalSessionState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        self.session_meta = derive_session_meta(&self.entries, jsonl_path.to_path_buf())?;
        Ok(())
    }

    fn require_existing_entry(&self, leaf_id: &str) -> Result<(), SessionStoreError> {
        if self.entry_ids.contains(leaf_id) {
            Ok(())
        } else {
            Err(SessionStoreError::IndexInconsistent {
                message: format!("leaf target `{leaf_id}` does not exist"),
            })
        }
    }
}

/// `InMemorySessionStore` 为运行时测试提供不落盘的 mock 实现。
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<SessionId, InMemorySession>>,
}

struct InMemorySession {
    entries: Vec<SessionEntry>,
    jsonl_path: PathBuf,
}

impl InMemorySessionStore {
    /// 创建空的内存 session store。
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore for InMemorySessionStore {
    async fn create_session(&self, header: SessionHeader) -> Result<SessionId, SessionStoreError> {
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
    }

    async fn append(
        &self,
        session_id: &SessionId,
        item: ConversationItem,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let mut sessions = self.sessions.write().await;
        let session =
            sessions
                .get_mut(&session_id)
                .ok_or_else(|| SessionStoreError::SessionNotFound {
                    session_id: session_id.clone(),
                })?;
        let entry = SessionEntry {
            id: generate_entry_id(&entry_ids(&session.entries)),
            parent_id: append_parent_id(&session.entries),
            timestamp: current_timestamp_ms()?,
            kind: SessionEntryKind::Item(item),
        };
        session.entries.push(entry);
        Ok(())
    }

    async fn set_leaf(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let requested_leaf_id = leaf_id.map(str::to_string);
        let mut sessions = self.sessions.write().await;
        let session =
            sessions
                .get_mut(&session_id)
                .ok_or_else(|| SessionStoreError::SessionNotFound {
                    session_id: session_id.clone(),
                })?;
        if let Some(leaf_id) = requested_leaf_id.as_deref()
            && !session.entries.iter().any(|entry| entry.id == leaf_id)
        {
            return Err(SessionStoreError::IndexInconsistent {
                message: format!("leaf target `{leaf_id}` does not exist"),
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
    }

    async fn resolve(
        &self,
        session_id: &SessionId,
        leaf_id: Option<&str>,
    ) -> Result<Vec<ConversationItem>, SessionStoreError> {
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
    }

    async fn list_sessions(
        &self,
        project_dir: &str,
    ) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let project_dir = normalize_project_dir(Path::new(project_dir));
        let sessions = self.sessions.read().await;
        let mut metas = sessions
            .values()
            .map(|session| derive_session_meta(&session.entries, session.jsonl_path.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        metas.retain(|meta| meta.project_dir == project_dir);
        metas.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.created_at.cmp(&left.created_at))
                .then_with(|| right.session_id.cmp(&left.session_id))
        });
        Ok(metas)
    }

    async fn get_session_meta(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionMeta, SessionStoreError> {
        let session_id = session_id.clone();
        let sessions = self.sessions.read().await;
        let session =
            sessions
                .get(&session_id)
                .ok_or_else(|| SessionStoreError::SessionNotFound {
                    session_id: session_id.clone(),
                })?;
        derive_session_meta(&session.entries, session.jsonl_path.clone())
    }

    async fn flush(&self, session_id: &SessionId) -> Result<(), SessionStoreError> {
        let session_id = session_id.clone();
        let sessions = self.sessions.read().await;
        if sessions.contains_key(&session_id) {
            Ok(())
        } else {
            Err(SessionStoreError::SessionNotFound { session_id })
        }
    }
}

fn session_jsonl_path(hunea_dir: &Path, work_dir: &Path, session_id: &SessionId) -> PathBuf {
    hunea_dir
        .join("sessions")
        .join(encode_project_dir(work_dir))
        .join(session_filename(session_id))
}

fn requested_leaf_id<'a>(
    entries: &'a [SessionEntry],
    leaf_id: Option<&'a str>,
) -> Result<&'a str, SessionStoreError> {
    if let Some(leaf_id) = leaf_id {
        return Ok(leaf_id);
    }

    entries
        .last()
        .map(|entry| entry.id.as_str())
        .ok_or_else(|| SessionStoreError::IndexInconsistent {
            message: "session is missing persisted entries".to_string(),
        })
}

fn append_parent_id(entries: &[SessionEntry]) -> Option<String> {
    match entries.last().map(|entry| &entry.kind) {
        Some(SessionEntryKind::Leaf {
            target_id: Some(target_id),
        }) => Some(target_id.clone()),
        Some(SessionEntryKind::Leaf { target_id: None }) => latest_non_leaf_id(entries),
        Some(_) => entries.last().map(|entry| entry.id.clone()),
        None => None,
    }
}

fn latest_non_leaf_id(entries: &[SessionEntry]) -> Option<String> {
    entries
        .iter()
        .rev()
        .find(|entry| !matches!(entry.kind, SessionEntryKind::Leaf { .. }))
        .map(|entry| entry.id.clone())
}

fn entry_ids(entries: &[SessionEntry]) -> HashSet<String> {
    entries.iter().map(|entry| entry.id.clone()).collect()
}

fn derive_session_meta(
    entries: &[SessionEntry],
    jsonl_path: PathBuf,
) -> Result<SessionMeta, SessionStoreError> {
    let mut header_entry = None;
    let mut first_user_message = None;
    let mut latest_user_message = None;
    let mut latest_model = None;

    for entry in entries {
        match &entry.kind {
            SessionEntryKind::Header(header) if header_entry.is_none() => {
                header_entry = Some((header.clone(), entry.timestamp));
            }
            SessionEntryKind::Item(item) if item.role() == Some(Role::User) => {
                let text = item.text_content();
                if first_user_message.is_none() {
                    first_user_message = Some(text.clone());
                }
                latest_user_message = Some(text);
            }
            SessionEntryKind::ConfigChange(snapshot) => {
                latest_model = Some(snapshot.model.clone());
            }
            _ => {}
        }
    }

    let (header, created_at) =
        header_entry.ok_or_else(|| SessionStoreError::IndexInconsistent {
            message: "session is missing header entry".to_string(),
        })?;
    let updated_at = entries
        .last()
        .map(|entry| entry.timestamp)
        .unwrap_or(created_at);
    let title = header
        .session_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            first_user_message
                .as_deref()
                .map(|text| truncate_chars(text, 50))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| header.session_id.to_string());
    let preview = latest_user_message
        .as_deref()
        .map(|text| truncate_chars(text, 100))
        .filter(|value| !value.is_empty());

    Ok(SessionMeta {
        session_id: header.session_id.clone(),
        project_dir: normalize_project_dir(&header.work_dir),
        title,
        preview,
        total_tokens: 0,
        model: latest_model.or_else(|| Some(header.initial_model.clone())),
        created_at,
        updated_at,
        git_head: header.git_head.clone(),
        work_dir: header.work_dir.clone(),
        jsonl_path,
    })
}

fn normalize_project_dir(work_dir: &Path) -> String {
    work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn current_timestamp_ms() -> Result<i64, SessionStoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| SessionStoreError::IndexInconsistent {
            message: format!("system time is before unix epoch: {error}"),
        })?;
    i64::try_from(duration.as_millis()).map_err(|_| SessionStoreError::IndexInconsistent {
        message: "system time exceeds i64 millisecond range".to_string(),
    })
}

fn resolve_error(error: ResolveError) -> SessionStoreError {
    match error {
        ResolveError::DuplicateId(id) => SessionStoreError::DuplicateId { id },
        ResolveError::DanglingParent(parent_id) => SessionStoreError::DanglingParent { parent_id },
        ResolveError::LeafNotFound(leaf_id) => SessionStoreError::IndexInconsistent {
            message: format!("resolve failed because leaf `{leaf_id}` was not found"),
        },
        ResolveError::CycleDetected => SessionStoreError::IndexInconsistent {
            message: "resolve failed because the entry graph contains a cycle".to_string(),
        },
        ResolveError::InvalidCompactionTarget(target_id) => SessionStoreError::IndexInconsistent {
            message: format!("resolve failed because compaction target `{target_id}` is invalid"),
        },
    }
}

async fn load_entries(path: &Path) -> Result<Vec<SessionEntry>, SessionStoreError> {
    let path = path.to_path_buf();
    task::spawn_blocking(move || JsonlLoader::load(&path))
        .await
        .map_err(|_| SessionStoreError::MetadataTaskPanicked)?
}

fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}
