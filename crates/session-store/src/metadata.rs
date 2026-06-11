#![cfg_attr(not(test), allow(dead_code))]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
    time::UNIX_EPOCH,
};

use provider_protocol::Role;
use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use tokio::task;

use crate::{
    SESSION_MESSAGE_PREVIEW_CHAR_LIMIT, SESSION_TITLE_FALLBACK_CHAR_LIMIT, SessionEntryKind,
    SessionMeta, SessionStoreError, SessionStoreError::IndexInconsistent, encode_project_dir,
    jsonl::JsonlLoader,
};

const SQLITE_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SQLITE_INIT_RETRY_ATTEMPTS: usize = 5;
const SQLITE_INIT_RETRY_DELAY: Duration = Duration::from_millis(50);

/// `MetadataIndex` 管理 session 的 SQLite 元数据索引。
pub(crate) struct MetadataIndex {
    index_path: Arc<PathBuf>,
    sessions_dir: Arc<PathBuf>,
}

impl MetadataIndex {
    pub(crate) async fn open(path: &Path) -> Result<Self, SessionStoreError> {
        let index_path = path.to_path_buf();
        let sessions_dir = path
            .parent()
            .map(|parent| parent.join("sessions"))
            .unwrap_or_else(|| PathBuf::from("sessions"));
        let init_path = index_path.clone();

        spawn_index_task(move || initialize_database_with_retry(&init_path)).await?;

        Ok(Self {
            index_path: Arc::new(index_path),
            sessions_dir: Arc::new(sessions_dir),
        })
    }

    pub(crate) async fn upsert_session(&self, meta: &SessionMeta) -> Result<(), SessionStoreError> {
        let meta = meta.clone();
        self.run_blocking(move |index_path, _| {
            with_connection(&index_path, |conn| upsert_session_row(conn, &meta))
        })
        .await
    }

    pub(crate) async fn list_sessions(
        &self,
        project_dir: &str,
    ) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let project_dir = project_dir.to_string();
        self.run_blocking(move |index_path, sessions_dir| {
            with_connection(&index_path, |conn| {
                repair_project_from_jsonl(conn, &sessions_dir, &project_dir)?;
                list_session_rows(conn, &project_dir)
            })
        })
        .await
    }

    pub(crate) async fn get_session_meta(
        &self,
        session_id: &str,
    ) -> Result<SessionMeta, SessionStoreError> {
        let session_id = session_id.to_string();
        self.run_blocking(move |index_path, _| {
            with_connection(&index_path, |conn| get_session_meta_row(conn, &session_id))
        })
        .await
    }

    pub(crate) async fn backfill_from_jsonl(
        &self,
        sessions_dir: &Path,
    ) -> Result<usize, SessionStoreError> {
        let sessions_dir = sessions_dir.to_path_buf();
        self.run_blocking(move |index_path, _| {
            with_connection(&index_path, |conn| {
                rebuild_index_from_jsonl(conn, &sessions_dir)
            })
        })
        .await
    }

    async fn run_blocking<T>(
        &self,
        operation: impl FnOnce(PathBuf, PathBuf) -> Result<T, SessionStoreError> + Send + 'static,
    ) -> Result<T, SessionStoreError>
    where
        T: Send + 'static,
    {
        let index_path = (*self.index_path).clone();
        let sessions_dir = (*self.sessions_dir).clone();
        spawn_index_task(move || operation(index_path, sessions_dir)).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionFileFingerprint {
    file_size: u64,
    modified_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DiscoveredSessionFile {
    path: PathBuf,
    fingerprint: SessionFileFingerprint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexedProjectFile {
    session_id: String,
    jsonl_path: PathBuf,
    fingerprint: Option<SessionFileFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractedSessionMeta {
    meta: SessionMeta,
    fingerprint: SessionFileFingerprint,
}

#[derive(Debug, PartialEq, Eq)]
struct RepairPlan {
    stale_session_ids: Vec<String>,
    files_to_refresh: Vec<DiscoveredSessionFile>,
}

async fn spawn_index_task<T>(
    operation: impl FnOnce() -> Result<T, SessionStoreError> + Send + 'static,
) -> Result<T, SessionStoreError>
where
    T: Send + 'static,
{
    task::spawn_blocking(operation)
        .await
        .map_err(|_| SessionStoreError::MetadataTaskPanicked)?
}

fn initialize_database(index_path: &Path) -> Result<(), SessionStoreError> {
    with_connection(index_path, initialize_database_schema)
}

fn initialize_database_with_retry(index_path: &Path) -> Result<(), SessionStoreError> {
    for attempt in 0..SQLITE_INIT_RETRY_ATTEMPTS {
        match initialize_database(index_path) {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt + 1 < SQLITE_INIT_RETRY_ATTEMPTS && is_sqlite_busy_error(&error) =>
            {
                thread::sleep(SQLITE_INIT_RETRY_DELAY);
            }
            Err(error) => return Err(error),
        }
    }

    unreachable!("retry loop should return before exhausting attempts")
}

fn with_connection<T>(
    index_path: &Path,
    operation: impl FnOnce(&Connection) -> Result<T, SessionStoreError>,
) -> Result<T, SessionStoreError> {
    if let Some(parent_dir) = index_path.parent() {
        fs::create_dir_all(parent_dir).map_err(io_error)?;
    }

    let conn = Connection::open(index_path).map_err(sqlite_error)?;
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(sqlite_error)?;
    enable_wal_mode(&conn)?;
    initialize_database_schema(&conn)?;

    operation(&conn)
}

fn enable_wal_mode(conn: &Connection) -> Result<(), SessionStoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .map_err(sqlite_error)?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(IndexInconsistent {
            message: format!("sqlite journal_mode is `{journal_mode}`, expected `wal`"),
        });
    }

    Ok(())
}

fn initialize_database_schema(conn: &Connection) -> Result<(), SessionStoreError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS sessions (
            session_id TEXT PRIMARY KEY,
            project_dir TEXT NOT NULL,
            title TEXT NOT NULL,
            preview TEXT,
            first_user_preview TEXT,
            last_assistant_preview TEXT,
            total_tokens INTEGER NOT NULL DEFAULT 0,
            model TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            git_head TEXT,
            work_dir TEXT NOT NULL,
            jsonl_path TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_sessions_updated_at
        ON sessions(updated_at DESC);

        CREATE INDEX IF NOT EXISTS idx_sessions_project_dir
        ON sessions(project_dir);

        CREATE TABLE IF NOT EXISTS session_repair_state (
            session_id TEXT PRIMARY KEY,
            jsonl_path TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_at_ms INTEGER NOT NULL
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_session_repair_state_jsonl_path
        ON session_repair_state(jsonl_path);
        ",
    )
    .map_err(sqlite_error)
}

fn upsert_session_row(conn: &Connection, meta: &SessionMeta) -> Result<(), SessionStoreError> {
    conn.execute(
        "
        INSERT INTO sessions (
            session_id,
            project_dir,
            title,
            preview,
            first_user_preview,
            last_assistant_preview,
            total_tokens,
            model,
            created_at,
            updated_at,
            git_head,
            work_dir,
            jsonl_path
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(session_id) DO UPDATE SET
            project_dir = excluded.project_dir,
            title = excluded.title,
            preview = excluded.preview,
            first_user_preview = excluded.first_user_preview,
            last_assistant_preview = excluded.last_assistant_preview,
            total_tokens = excluded.total_tokens,
            model = excluded.model,
            created_at = excluded.created_at,
            updated_at = excluded.updated_at,
            git_head = excluded.git_head,
            work_dir = excluded.work_dir,
            jsonl_path = excluded.jsonl_path
        ",
        params![
            meta.session_id.to_string(),
            meta.project_dir,
            meta.title,
            meta.preview,
            meta.first_user_preview,
            meta.last_assistant_preview,
            checked_i64(
                meta.total_tokens,
                &format!("session `{}` total_tokens", meta.session_id)
            )?,
            meta.model,
            meta.created_at,
            meta.updated_at,
            meta.git_head,
            meta.work_dir.to_string_lossy(),
            meta.jsonl_path.to_string_lossy(),
        ],
    )
    .map_err(sqlite_error)?;

    Ok(())
}

fn upsert_repair_state(
    conn: &Connection,
    session_id: &str,
    jsonl_path: &Path,
    fingerprint: &SessionFileFingerprint,
) -> Result<(), SessionStoreError> {
    conn.execute(
        "
        INSERT INTO session_repair_state (
            session_id,
            jsonl_path,
            file_size,
            modified_at_ms
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(session_id) DO UPDATE SET
            jsonl_path = excluded.jsonl_path,
            file_size = excluded.file_size,
            modified_at_ms = excluded.modified_at_ms
        ",
        params![
            session_id,
            jsonl_path.to_string_lossy(),
            checked_i64(
                fingerprint.file_size,
                &format!("session `{session_id}` file_size")
            )?,
            fingerprint.modified_at_ms,
        ],
    )
    .map_err(sqlite_error)?;

    Ok(())
}

fn get_session_meta_row(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionMeta, SessionStoreError> {
    let requested_session_id =
        session_id
            .parse()
            .map_err(|_| SessionStoreError::IndexInconsistent {
                message: format!("session id `{session_id}` is not a valid UUIDv7"),
            })?;
    let meta = conn
        .query_row(
            "
            SELECT
                session_id,
                project_dir,
                title,
                preview,
                first_user_preview,
                last_assistant_preview,
                total_tokens,
                model,
                created_at,
                updated_at,
                git_head,
                work_dir,
                jsonl_path
            FROM sessions
            WHERE session_id = ?1
            ",
            params![session_id],
            row_to_session_meta,
        )
        .optional()
        .map_err(sqlite_error)?;

    meta.ok_or(SessionStoreError::SessionNotFound {
        session_id: requested_session_id,
    })
}

fn list_session_rows(
    conn: &Connection,
    project_dir: &str,
) -> Result<Vec<SessionMeta>, SessionStoreError> {
    let mut statement = conn
        .prepare(
            "
            SELECT
                session_id,
                project_dir,
                title,
                preview,
                first_user_preview,
                last_assistant_preview,
                total_tokens,
                model,
                created_at,
                updated_at,
                git_head,
                work_dir,
                jsonl_path
            FROM sessions
            WHERE project_dir = ?1
            ORDER BY updated_at DESC, created_at DESC, session_id DESC
            ",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![project_dir], row_to_session_meta)
        .map_err(sqlite_error)?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(sqlite_error)?);
    }

    Ok(sessions)
}

fn rebuild_index_from_jsonl(
    conn: &Connection,
    sessions_dir: &Path,
) -> Result<usize, SessionStoreError> {
    conn.execute("DELETE FROM session_repair_state", [])
        .map_err(sqlite_error)?;
    conn.execute("DELETE FROM sessions", [])
        .map_err(sqlite_error)?;

    let mut processed = 0;
    for discovered_file in collect_jsonl_files(sessions_dir)? {
        let extracted = extract_session_meta(&discovered_file)?;
        upsert_session_row(conn, &extracted.meta)?;
        upsert_repair_state(
            conn,
            &extracted.meta.session_id.to_string(),
            &extracted.meta.jsonl_path,
            &extracted.fingerprint,
        )?;
        processed += 1;
    }

    Ok(processed)
}

fn repair_project_from_jsonl(
    conn: &Connection,
    sessions_dir: &Path,
    project_dir: &str,
) -> Result<(), SessionStoreError> {
    let project_sessions_dir = sessions_dir.join(encode_project_dir(Path::new(project_dir)));
    let discovered_files = collect_jsonl_files(&project_sessions_dir)?;
    let indexed_files = load_indexed_project_files(conn, project_dir)?;
    let repair_plan = build_repair_plan(&discovered_files, &indexed_files);
    let indexed_by_path: BTreeMap<PathBuf, IndexedProjectFile> = indexed_files
        .into_iter()
        .map(|indexed| (indexed.jsonl_path.clone(), indexed))
        .collect();

    for session_id in repair_plan.stale_session_ids {
        delete_session_rows(conn, &session_id)?;
    }

    for discovered_file in repair_plan.files_to_refresh {
        let extracted = extract_session_meta(&discovered_file)?;
        if let Some(existing) = indexed_by_path.get(&discovered_file.path)
            && existing.session_id != extracted.meta.session_id.to_string()
        {
            delete_session_rows(conn, &existing.session_id)?;
        }
        upsert_session_row(conn, &extracted.meta)?;
        upsert_repair_state(
            conn,
            &extracted.meta.session_id.to_string(),
            &extracted.meta.jsonl_path,
            &extracted.fingerprint,
        )?;
    }

    Ok(())
}

fn delete_session_rows(conn: &Connection, session_id: &str) -> Result<(), SessionStoreError> {
    conn.execute(
        "DELETE FROM session_repair_state WHERE session_id = ?1",
        params![session_id],
    )
    .map_err(sqlite_error)?;
    conn.execute(
        "DELETE FROM sessions WHERE session_id = ?1",
        params![session_id],
    )
    .map_err(sqlite_error)?;
    Ok(())
}

fn load_indexed_project_files(
    conn: &Connection,
    project_dir: &str,
) -> Result<Vec<IndexedProjectFile>, SessionStoreError> {
    let mut statement = conn
        .prepare(
            "
            SELECT
                s.session_id,
                s.jsonl_path,
                r.file_size,
                r.modified_at_ms
            FROM sessions s
            LEFT JOIN session_repair_state r ON r.session_id = s.session_id
            WHERE s.project_dir = ?1
            ",
        )
        .map_err(sqlite_error)?;
    let rows = statement
        .query_map(params![project_dir], |row| {
            let file_size = row.get::<_, Option<i64>>(2)?;
            let modified_at_ms = row.get::<_, Option<i64>>(3)?;
            Ok(IndexedProjectFile {
                session_id: row.get(0)?,
                jsonl_path: PathBuf::from(row.get::<_, String>(1)?),
                fingerprint: match (file_size, modified_at_ms) {
                    (Some(file_size), Some(modified_at_ms)) => Some(SessionFileFingerprint {
                        file_size: u64::try_from(file_size).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        modified_at_ms,
                    }),
                    _ => None,
                },
            })
        })
        .map_err(sqlite_error)?;

    let mut indexed_files = Vec::new();
    for row in rows {
        indexed_files.push(row.map_err(sqlite_error)?);
    }

    Ok(indexed_files)
}

fn build_repair_plan(
    discovered_files: &[DiscoveredSessionFile],
    indexed_files: &[IndexedProjectFile],
) -> RepairPlan {
    let discovered_by_path: BTreeMap<&Path, &DiscoveredSessionFile> = discovered_files
        .iter()
        .map(|file| (file.path.as_path(), file))
        .collect();
    let indexed_by_path: BTreeMap<&Path, &IndexedProjectFile> = indexed_files
        .iter()
        .map(|indexed| (indexed.jsonl_path.as_path(), indexed))
        .collect();

    let stale_session_ids = indexed_files
        .iter()
        .filter(|indexed| !discovered_by_path.contains_key(indexed.jsonl_path.as_path()))
        .map(|indexed| indexed.session_id.clone())
        .collect();
    let files_to_refresh = discovered_files
        .iter()
        .filter(
            |discovered| match indexed_by_path.get(discovered.path.as_path()) {
                Some(indexed) => indexed.fingerprint.as_ref() != Some(&discovered.fingerprint),
                None => true,
            },
        )
        .cloned()
        .collect();

    RepairPlan {
        stale_session_ids,
        files_to_refresh,
    }
}

fn extract_session_meta(
    discovered_file: &DiscoveredSessionFile,
) -> Result<ExtractedSessionMeta, SessionStoreError> {
    let mut header_entry: Option<(crate::SessionHeader, i64)> = None;
    let mut first_user_message = None;
    let mut latest_user_message = None;
    let mut latest_assistant_message = None;
    let mut latest_model = None;
    let mut updated_at = None;

    JsonlLoader::scan(&discovered_file.path, |entry| {
        updated_at = Some(entry.timestamp);

        match entry.kind {
            SessionEntryKind::Header(header) if header_entry.is_none() => {
                header_entry = Some((header, entry.timestamp));
            }
            SessionEntryKind::Header(_) => {}
            SessionEntryKind::Item(item) if item.role() == Some(Role::User) => {
                let text = item.text_content();
                if first_user_message.is_none() {
                    first_user_message = Some(text.clone());
                }
                latest_user_message = Some(text);
            }
            SessionEntryKind::Item(item) if item.role() == Some(Role::Assistant) => {
                latest_assistant_message = Some(item.text_content());
            }
            SessionEntryKind::ConfigChange(snapshot) => {
                latest_model = Some(snapshot.model);
            }
            _ => {}
        }

        Ok(())
    })?;

    let (header, created_at) = header_entry.ok_or_else(|| IndexInconsistent {
        message: format!(
            "session file `{}` is missing a header entry",
            discovered_file.path.display()
        ),
    })?;
    let title = header
        .session_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            first_user_message
                .as_deref()
                .map(|text| truncate_chars(text, SESSION_TITLE_FALLBACK_CHAR_LIMIT))
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| header.session_id.to_string());
    let preview = latest_user_message
        .as_deref()
        .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
        .filter(|value| !value.is_empty());
    let first_user_preview = first_user_message
        .as_deref()
        .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
        .filter(|value| !value.is_empty());
    let last_assistant_preview = latest_assistant_message
        .as_deref()
        .map(|text| truncate_chars(text, SESSION_MESSAGE_PREVIEW_CHAR_LIMIT))
        .filter(|value| !value.is_empty());
    let meta = SessionMeta {
        session_id: header.session_id.clone(),
        project_dir: normalize_project_dir(&header.work_dir),
        title,
        preview,
        first_user_preview,
        last_assistant_preview,
        total_tokens: 0,
        model: latest_model.or_else(|| Some(header.initial_model.clone())),
        created_at,
        updated_at: updated_at.unwrap_or(created_at),
        git_head: header.git_head.clone(),
        work_dir: header.work_dir.clone(),
        jsonl_path: discovered_file.path.clone(),
    };

    Ok(ExtractedSessionMeta {
        meta,
        fingerprint: discovered_file.fingerprint.clone(),
    })
}

fn collect_jsonl_files(directory: &Path) -> Result<Vec<DiscoveredSessionFile>, SessionStoreError> {
    let mut files = Vec::new();
    collect_jsonl_files_into(directory, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

fn collect_jsonl_files_into(
    directory: &Path,
    files: &mut Vec<DiscoveredSessionFile>,
) -> Result<(), SessionStoreError> {
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io_error(error)),
    };

    for entry in read_dir {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(io_error)?;
        if file_type.is_dir() {
            collect_jsonl_files_into(&path, files)?;
            continue;
        }
        if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(DiscoveredSessionFile {
                fingerprint: file_fingerprint(&path)?,
                path,
            });
        }
    }

    Ok(())
}

fn file_fingerprint(path: &Path) -> Result<SessionFileFingerprint, SessionStoreError> {
    let metadata = fs::metadata(path).map_err(io_error)?;
    Ok(SessionFileFingerprint {
        file_size: metadata.len(),
        modified_at_ms: modified_time_ms(&metadata)?,
    })
}

fn modified_time_ms(metadata: &fs::Metadata) -> Result<i64, SessionStoreError> {
    let duration = metadata
        .modified()
        .map_err(io_error)?
        .duration_since(UNIX_EPOCH)
        .map_err(|error| IndexInconsistent {
            message: format!("file modified time is before unix epoch: {error}"),
        })?;
    i64::try_from(duration.as_millis()).map_err(|_| IndexInconsistent {
        message: "file modified time exceeds i64 range".to_string(),
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

fn checked_i64(value: u64, label: &str) -> Result<i64, SessionStoreError> {
    i64::try_from(value).map_err(|_| IndexInconsistent {
        message: format!("{label} exceeds sqlite INTEGER range"),
    })
}

fn row_to_session_meta(row: &rusqlite::Row<'_>) -> Result<SessionMeta, rusqlite::Error> {
    let total_tokens = row.get::<_, i64>(6)?;
    let session_id_text: String = row.get(0)?;

    Ok(SessionMeta {
        session_id: session_id_text.parse().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        project_dir: row.get(1)?,
        title: row.get(2)?,
        preview: row.get(3)?,
        first_user_preview: row.get(4)?,
        last_assistant_preview: row.get(5)?,
        total_tokens: u64::try_from(total_tokens).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                6,
                rusqlite::types::Type::Integer,
                Box::new(error),
            )
        })?,
        model: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        git_head: row.get(10)?,
        work_dir: PathBuf::from(row.get::<_, String>(11)?),
        jsonl_path: PathBuf::from(row.get::<_, String>(12)?),
    })
}

fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}

fn sqlite_error(source: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::SqliteError { source }
}

fn is_sqlite_busy_error(error: &SessionStoreError) -> bool {
    matches!(
        error,
        SessionStoreError::SqliteError { source }
            if source.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
    };

    use provider_protocol::{ConversationItem, Role};
    use rusqlite::Connection;
    use uuid::Uuid;

    use crate::{
        SessionEntry, SessionEntryKind, SessionHeader, SessionId, SessionMeta, encode_project_dir,
        jsonl::JsonlWriter, metadata::MetadataIndex, session_filename,
    };

    use super::{
        DiscoveredSessionFile, IndexedProjectFile, SessionFileFingerprint, build_repair_plan,
        initialize_database,
    };

    #[tokio::test]
    async fn metadata_index_roundtrips_one_session() {
        let root = tempdir_path("metadata-index-roundtrip");
        fs::create_dir_all(&root).expect("temp root should be creatable");
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");
        let meta = sample_session_meta();

        index
            .upsert_session(&meta)
            .await
            .expect("upsert should persist session metadata");

        let loaded = index
            .get_session_meta(&meta.session_id.to_string())
            .await
            .expect("session metadata should be queryable by id");

        assert_eq!(loaded, meta);

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn list_sessions_orders_results_by_updated_at_descending() {
        let root = tempdir_path("metadata-index-order");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");
        let earliest_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let middle_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
            .parse()
            .expect("fixture session id should parse");
        let latest_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ad"
            .parse()
            .expect("fixture session id should parse");
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &earliest_id,
            session_fixture_entries(&work_dir, &earliest_id, 10, "first"),
        );
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &middle_id,
            session_fixture_entries(&work_dir, &middle_id, 20, "second"),
        );
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &latest_id,
            session_fixture_entries(&work_dir, &latest_id, 30, "third"),
        );
        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("backfill should load ordered fixtures");

        let listed = index
            .list_sessions(
                &work_dir
                    .canonicalize()
                    .expect("work dir should be canonicalizable")
                    .to_string_lossy(),
            )
            .await
            .expect("session list should be queryable");

        assert_eq!(
            listed
                .into_iter()
                .map(|meta| meta.session_id.to_string())
                .collect::<Vec<_>>(),
            vec![
                latest_id.to_string(),
                middle_id.to_string(),
                earliest_id.to_string(),
            ]
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn list_sessions_filters_by_project_dir() {
        let root = tempdir_path("metadata-index-project-filter");
        let sessions_dir = root.join("sessions");
        let repo_a = root.join("workspace").join("repo-a");
        let repo_b = root.join("workspace").join("repo-b");
        fs::create_dir_all(&repo_a).expect("repo A should be creatable");
        fs::create_dir_all(&repo_b).expect("repo B should be creatable");
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");
        let repo_a_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let repo_b_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
            .parse()
            .expect("fixture session id should parse");
        write_session_jsonl(
            &sessions_dir,
            &repo_a,
            &repo_a_id,
            session_fixture_entries(&repo_a, &repo_a_id, 10, "repo-a"),
        );
        write_session_jsonl(
            &sessions_dir,
            &repo_b,
            &repo_b_id,
            session_fixture_entries(&repo_b, &repo_b_id, 20, "repo-b"),
        );
        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("backfill should load both projects");

        let listed = index
            .list_sessions(
                &repo_a
                    .canonicalize()
                    .expect("repo A should be canonicalizable")
                    .to_string_lossy(),
            )
            .await
            .expect("session list should be filtered by project");

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, repo_a_id);

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn backfill_from_jsonl_derives_session_metadata() {
        let root = tempdir_path("metadata-index-backfill");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let jsonl_path = write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &session_id,
            vec![
                SessionEntry {
                    id: "header".to_string(),
                    parent_id: None,
                    timestamp: 1_717_514_800_000,
                    kind: SessionEntryKind::Header(SessionHeader {
                        session_id: session_id.clone(),
                        work_dir: work_dir.clone(),
                        session_name: None,
                        initial_model: "gpt-4.1".to_string(),
                        git_head: Some("abc123".to_string()),
                        cli_version: Some("0.5.5".to_string()),
                    }),
                },
                SessionEntry {
                    id: "user-1".to_string(),
                    parent_id: Some("header".to_string()),
                    timestamp: 1_717_514_800_050,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::User,
                        "Please inspect src/main.rs and explain startup wiring in detail.",
                    )),
                },
                SessionEntry {
                    id: "assistant-1".to_string(),
                    parent_id: Some("user-1".to_string()),
                    timestamp: 1_717_514_800_075,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::Assistant,
                        "The startup path is straightforward.",
                    )),
                },
                SessionEntry {
                    id: "config-1".to_string(),
                    parent_id: Some("assistant-1".to_string()),
                    timestamp: 1_717_514_800_090,
                    kind: SessionEntryKind::ConfigChange(crate::ConfigSnapshot {
                        model: "gpt-4.1-mini".to_string(),
                        system_prompt: None,
                    }),
                },
                SessionEntry {
                    id: "user-2".to_string(),
                    parent_id: Some("config-1".to_string()),
                    timestamp: 1_717_514_800_100,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::User,
                        "Add persistence hooks after that.",
                    )),
                },
            ],
        );
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");

        let processed = index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("backfill should parse session jsonl");
        let loaded = index
            .get_session_meta(&session_id.to_string())
            .await
            .expect("backfilled metadata should be queryable");

        assert_eq!(processed, 1);
        assert_eq!(loaded.session_id, session_id);
        assert_eq!(
            loaded.project_dir,
            work_dir
                .canonicalize()
                .expect("work dir should be canonicalizable")
                .to_string_lossy()
        );
        assert_eq!(
            loaded.title,
            "Please inspect src/main.rs and explain startup wir"
        );
        assert_eq!(
            loaded.preview.as_deref(),
            Some("Add persistence hooks after that.")
        );
        assert_eq!(
            loaded.first_user_preview.as_deref(),
            Some("Please inspect src/main.rs and explain startup wiring in detail.")
        );
        assert_eq!(
            loaded.last_assistant_preview.as_deref(),
            Some("The startup path is straightforward.")
        );
        assert_eq!(loaded.model.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(loaded.created_at, 1_717_514_800_000);
        assert_eq!(loaded.updated_at, 1_717_514_800_100);
        assert_eq!(loaded.git_head.as_deref(), Some("abc123"));
        assert_eq!(loaded.work_dir, work_dir);
        assert_eq!(loaded.jsonl_path, jsonl_path);

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn backfill_keeps_session_message_previews_to_256_chars() {
        let root = tempdir_path("metadata-index-long-previews");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let long_user_message = "u".repeat(320);
        let long_assistant_message = "a".repeat(320);
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &session_id,
            vec![
                SessionEntry {
                    id: "header".to_string(),
                    parent_id: None,
                    timestamp: 1_717_514_800_000,
                    kind: SessionEntryKind::Header(SessionHeader {
                        session_id: session_id.clone(),
                        work_dir: work_dir.clone(),
                        session_name: None,
                        initial_model: "gpt-4.1".to_string(),
                        git_head: None,
                        cli_version: None,
                    }),
                },
                SessionEntry {
                    id: "user-1".to_string(),
                    parent_id: Some("header".to_string()),
                    timestamp: 1_717_514_800_050,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::User,
                        long_user_message,
                    )),
                },
                SessionEntry {
                    id: "assistant-1".to_string(),
                    parent_id: Some("user-1".to_string()),
                    timestamp: 1_717_514_800_075,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::Assistant,
                        long_assistant_message,
                    )),
                },
            ],
        );
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");

        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("backfill should parse session jsonl");
        let loaded = index
            .get_session_meta(&session_id.to_string())
            .await
            .expect("backfilled metadata should be queryable");

        let expected_user_preview = "u".repeat(256);
        let expected_assistant_preview = "a".repeat(256);
        assert_eq!(
            loaded.first_user_preview.as_deref(),
            Some(expected_user_preview.as_str())
        );
        assert_eq!(
            loaded.last_assistant_preview.as_deref(),
            Some(expected_assistant_preview.as_str())
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn backfill_prefers_header_session_name_for_title_when_present() {
        let root = tempdir_path("metadata-index-session-name");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &session_id,
            vec![
                SessionEntry {
                    id: "header".to_string(),
                    parent_id: None,
                    timestamp: 1_717_514_800_000,
                    kind: SessionEntryKind::Header(SessionHeader {
                        session_id: session_id.clone(),
                        work_dir: work_dir.clone(),
                        session_name: Some("Debug persistence rollout".to_string()),
                        initial_model: "gpt-4.1".to_string(),
                        git_head: Some("abc123".to_string()),
                        cli_version: Some("0.5.5".to_string()),
                    }),
                },
                SessionEntry {
                    id: "user-1".to_string(),
                    parent_id: Some("header".to_string()),
                    timestamp: 1_717_514_800_050,
                    kind: SessionEntryKind::Item(ConversationItem::text(
                        Role::User,
                        "Fallback title should not be used.",
                    )),
                },
            ],
        );
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");

        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("backfill should parse session jsonl");
        let loaded = index
            .get_session_meta(&session_id.to_string())
            .await
            .expect("backfilled metadata should be queryable");

        assert_eq!(loaded.title, "Debug persistence rollout");

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn list_sessions_repairs_missing_sqlite_rows_from_jsonl() {
        let root = tempdir_path("metadata-index-read-repair-restore");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &session_id,
            session_fixture_entries(
                &work_dir,
                &session_id,
                1_717_514_800_050,
                "Repair me from jsonl.",
            ),
        );
        let index_path = root.join("index.sqlite");
        let index = MetadataIndex::open(&index_path)
            .await
            .expect("metadata index should open sqlite file");
        let project_dir = work_dir
            .canonicalize()
            .expect("work dir should be canonicalizable")
            .to_string_lossy()
            .into_owned();

        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("initial backfill should succeed");
        delete_session_row_for_test(&index_path, &session_id.to_string());

        let listed = index
            .list_sessions(&project_dir)
            .await
            .expect("list should repair missing sqlite rows");

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].session_id, session_id);

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn list_sessions_removes_orphaned_sqlite_rows_when_jsonl_is_deleted() {
        let root = tempdir_path("metadata-index-read-repair-prune");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let session_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let jsonl_path = write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &session_id,
            session_fixture_entries(
                &work_dir,
                &session_id,
                1_717_514_800_050,
                "Prune me when the jsonl is gone.",
            ),
        );
        let index = MetadataIndex::open(&root.join("index.sqlite"))
            .await
            .expect("metadata index should open sqlite file");
        let project_dir = work_dir
            .canonicalize()
            .expect("work dir should be canonicalizable")
            .to_string_lossy()
            .into_owned();

        index
            .backfill_from_jsonl(&sessions_dir)
            .await
            .expect("initial backfill should succeed");
        fs::remove_file(&jsonl_path).expect("fixture jsonl should be removable");

        let listed = index
            .list_sessions(&project_dir)
            .await
            .expect("list should prune orphaned sqlite rows");

        assert!(listed.is_empty());
        assert!(matches!(
            index.get_session_meta(&session_id.to_string()).await,
            Err(crate::SessionStoreError::SessionNotFound { .. })
        ));

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn backfill_rebuilds_metadata_after_sqlite_file_is_deleted() {
        let root = tempdir_path("metadata-index-rebuild");
        let sessions_dir = root.join("sessions");
        let work_dir = root.join("workspace").join("repo");
        fs::create_dir_all(&work_dir).expect("work dir should be creatable");
        let first_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ab"
            .parse()
            .expect("fixture session id should parse");
        let second_id: SessionId = "01914a5c-3c7e-7a2b-8abc-1234567890ac"
            .parse()
            .expect("fixture session id should parse");
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &first_id,
            session_fixture_entries(
                &work_dir,
                &first_id,
                1_717_514_800_050,
                "First rebuilt session.",
            ),
        );
        write_session_jsonl(
            &sessions_dir,
            &work_dir,
            &second_id,
            session_fixture_entries(
                &work_dir,
                &second_id,
                1_717_514_900_050,
                "Second rebuilt session.",
            ),
        );
        let index_path = root.join("index.sqlite");
        let project_dir = work_dir
            .canonicalize()
            .expect("work dir should be canonicalizable")
            .to_string_lossy()
            .into_owned();

        let before_delete = {
            let index = MetadataIndex::open(&index_path)
                .await
                .expect("metadata index should open");
            index
                .backfill_from_jsonl(&sessions_dir)
                .await
                .expect("initial backfill should succeed");
            index
                .list_sessions(&project_dir)
                .await
                .expect("session list should be queryable after backfill")
        };

        fs::remove_file(&index_path).expect("sqlite file should be removable");

        let after_rebuild = {
            let index = MetadataIndex::open(&index_path)
                .await
                .expect("metadata index should reopen");
            index
                .backfill_from_jsonl(&sessions_dir)
                .await
                .expect("rebuild backfill should succeed");
            index
                .list_sessions(&project_dir)
                .await
                .expect("session list should be queryable after rebuild")
        };

        assert_eq!(after_rebuild, before_delete);

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn multiple_metadata_indexes_can_write_different_sessions_without_lock_conflicts() {
        let root = tempdir_path("metadata-index-concurrency");
        fs::create_dir_all(&root).expect("temp root should be creatable");
        let index_path = root.join("index.sqlite");
        let first_meta = sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ab", 10);
        let second_meta = sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ac", 20);
        let first_id = first_meta.session_id.to_string();
        let second_id = second_meta.session_id.to_string();
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let first_future = async {
            let index = MetadataIndex::open(&index_path)
                .await
                .expect("first index should open");
            barrier.wait().await;
            index
                .upsert_session(&first_meta)
                .await
                .expect("first session should persist");
        };
        let second_future = async {
            let index = MetadataIndex::open(&index_path)
                .await
                .expect("second index should open");
            barrier.wait().await;
            index
                .upsert_session(&second_meta)
                .await
                .expect("second session should persist");
        };

        tokio::join!(first_future, second_future);

        let index = MetadataIndex::open(&index_path)
            .await
            .expect("verification index should open");
        assert_eq!(
            index
                .get_session_meta(&first_id)
                .await
                .expect("first session should exist")
                .session_id
                .to_string(),
            first_id
        );
        assert_eq!(
            index
                .get_session_meta(&second_id)
                .await
                .expect("second session should exist")
                .session_id
                .to_string(),
            second_id
        );

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[tokio::test]
    async fn open_enables_wal_mode() {
        let root = tempdir_path("metadata-index-wal");
        let index_path = root.join("index.sqlite");

        let _index = MetadataIndex::open(&index_path)
            .await
            .expect("metadata index should open sqlite file");

        let conn = Connection::open(&index_path).expect("sqlite file should be reopenable");
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .expect("journal mode should be queryable");

        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

        fs::remove_dir_all(root).expect("temp root should be removable");
    }

    #[test]
    fn repair_plan_skips_unchanged_files() {
        let unchanged = DiscoveredSessionFile {
            path: PathBuf::from("/sessions/a.jsonl"),
            fingerprint: SessionFileFingerprint {
                file_size: 128,
                modified_at_ms: 42,
            },
        };
        let changed = DiscoveredSessionFile {
            path: PathBuf::from("/sessions/b.jsonl"),
            fingerprint: SessionFileFingerprint {
                file_size: 256,
                modified_at_ms: 99,
            },
        };
        let indexed_files = vec![
            IndexedProjectFile {
                session_id: "a".to_string(),
                jsonl_path: unchanged.path.clone(),
                fingerprint: Some(unchanged.fingerprint.clone()),
            },
            IndexedProjectFile {
                session_id: "b".to_string(),
                jsonl_path: changed.path.clone(),
                fingerprint: Some(SessionFileFingerprint {
                    file_size: 255,
                    modified_at_ms: 99,
                }),
            },
            IndexedProjectFile {
                session_id: "stale".to_string(),
                jsonl_path: PathBuf::from("/sessions/missing.jsonl"),
                fingerprint: Some(SessionFileFingerprint {
                    file_size: 1,
                    modified_at_ms: 1,
                }),
            },
        ];

        let plan = build_repair_plan(&[unchanged, changed.clone()], &indexed_files);

        assert_eq!(plan.stale_session_ids, vec!["stale".to_string()]);
        assert_eq!(plan.files_to_refresh, vec![changed]);
    }

    fn sample_session_meta() -> SessionMeta {
        sample_session_meta_with("01914a5c-3c7e-7a2b-8abc-1234567890ab", 1_717_514_800_123)
    }

    fn sample_session_meta_with(session_id: &str, updated_at: i64) -> SessionMeta {
        let session_id: SessionId = session_id.parse().expect("fixture session id should parse");
        SessionMeta {
            session_id,
            project_dir: "/repo".to_string(),
            title: "Inspect session index".to_string(),
            preview: Some("please persist this metadata".to_string()),
            first_user_preview: Some("first user preview".to_string()),
            last_assistant_preview: Some("last assistant preview".to_string()),
            total_tokens: 512,
            model: Some("gpt-4.1".to_string()),
            created_at: 1_717_514_800_000,
            updated_at,
            git_head: Some("abc123".to_string()),
            work_dir: PathBuf::from("/repo"),
            jsonl_path: PathBuf::from("/tmp/session.jsonl"),
        }
    }

    fn tempdir_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("hunea-session-store-{label}-{}", Uuid::now_v7()))
    }

    fn write_session_jsonl(
        sessions_dir: &std::path::Path,
        work_dir: &std::path::Path,
        session_id: &SessionId,
        entries: Vec<SessionEntry>,
    ) -> PathBuf {
        let project_dir = sessions_dir.join(encode_project_dir(work_dir));
        fs::create_dir_all(&project_dir).expect("project dir should be creatable");
        let path = project_dir.join(session_filename(session_id));
        let mut writer = JsonlWriter::new(path.clone());
        writer
            .write_batch(&entries)
            .expect("fixture session jsonl should be writable");
        path
    }

    fn session_fixture_entries(
        work_dir: &std::path::Path,
        session_id: &SessionId,
        updated_at: i64,
        user_text: &str,
    ) -> Vec<SessionEntry> {
        vec![
            SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: updated_at - 1,
                kind: SessionEntryKind::Header(SessionHeader {
                    session_id: session_id.clone(),
                    work_dir: work_dir.to_path_buf(),
                    session_name: None,
                    initial_model: "gpt-4.1".to_string(),
                    git_head: Some("abc123".to_string()),
                    cli_version: Some("0.5.5".to_string()),
                }),
            },
            SessionEntry {
                id: "user-1".to_string(),
                parent_id: Some("header".to_string()),
                timestamp: updated_at,
                kind: SessionEntryKind::Item(ConversationItem::text(Role::User, user_text)),
            },
        ]
    }

    fn delete_session_row_for_test(index_path: &Path, session_id: &str) {
        initialize_database(index_path).expect("sqlite schema should initialize");
        let conn = Connection::open(index_path).expect("sqlite file should reopen");
        conn.execute(
            "DELETE FROM session_repair_state WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .expect("repair state row should be deletable");
        conn.execute(
            "DELETE FROM sessions WHERE session_id = ?1",
            rusqlite::params![session_id],
        )
        .expect("session row should be deletable");
    }
}
