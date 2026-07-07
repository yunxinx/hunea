use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::Duration,
};

use rusqlite::{Connection, ErrorCode, OptionalExtension, params};
use tokio::task;

use crate::{ProjectDir, SessionListOptions, SessionMeta, SessionStoreError};

mod repair;

const SQLITE_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SQLITE_INIT_RETRY_ATTEMPTS: usize = 5;
const SQLITE_INIT_RETRY_DELAY: Duration = Duration::from_millis(50);

/// `MetadataIndex` 管理 session 的 SQLite 元数据索引。
///
/// 列表查询默认只读取 SQLite，避免 UI 打开 session picker 时被 JSONL 扫盘阻塞。
/// 需要修复索引时，调用方必须显式传入 `SessionListOptions { repair: true }`
/// 或使用 `backfill_from_jsonl` 执行全量重建。
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
        project_dir: &ProjectDir,
        options: SessionListOptions,
    ) -> Result<Vec<SessionMeta>, SessionStoreError> {
        let project_dir = project_dir.clone();
        self.run_blocking(move |index_path, sessions_dir| {
            with_connection(&index_path, |conn| {
                if options.repair {
                    repair::repair_project_from_jsonl(conn, &sessions_dir, &project_dir)?;
                }
                list_session_rows(conn, &project_dir.canonical_string())
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
                repair::rebuild_index_from_jsonl(conn, &sessions_dir)
            })
        })
        .await
    }

    pub(crate) async fn record_message_history(
        &self,
        text: &str,
        limit: usize,
    ) -> Result<(), SessionStoreError> {
        let index_path = (*self.index_path).clone();
        let text = text.to_owned();
        spawn_index_task(move || {
            crate::message_history::record_message_history(&index_path, &text, limit)
        })
        .await
    }

    pub(crate) async fn load_message_history_recent(
        &self,
        limit: usize,
    ) -> Result<Vec<runtime_domain::session::MessageHistoryEntry>, SessionStoreError> {
        let index_path = (*self.index_path).clone();
        spawn_index_task(move || {
            crate::message_history::load_message_history_recent(&index_path, limit)
        })
        .await
    }

    pub(crate) async fn load_message_history_all(
        &self,
    ) -> Result<Vec<runtime_domain::session::MessageHistoryRow>, SessionStoreError> {
        let index_path = (*self.index_path).clone();
        spawn_index_task(move || crate::message_history::load_message_history_all(&index_path))
            .await
    }

    pub(crate) async fn save_global_prompt_assembly_state(
        &self,
        state: &runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState,
    ) -> Result<(), SessionStoreError> {
        let index_path = (*self.index_path).clone();
        let state = state.clone();
        spawn_index_task(move || {
            crate::prompt_assembly::save_global_prompt_assembly_state(&index_path, &state)
        })
        .await
    }

    pub(crate) async fn load_global_prompt_assembly_state(
        &self,
    ) -> Result<
        runtime_domain::prompt_assembly::persistence::PromptAssemblyScopeState,
        SessionStoreError,
    > {
        let index_path = (*self.index_path).clone();
        spawn_index_task(move || {
            crate::prompt_assembly::load_global_prompt_assembly_state(&index_path)
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
    with_connection(index_path, |conn| initialize_database_schema(conn))
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

    Err(SessionStoreError::CorruptIndex {
        message: "sqlite initialization retry loop exhausted without a result".to_string(),
    })
}

pub(crate) fn with_connection<T>(
    index_path: &Path,
    operation: impl FnOnce(&mut Connection) -> Result<T, SessionStoreError>,
) -> Result<T, SessionStoreError> {
    if let Some(parent_dir) = index_path.parent() {
        fs::create_dir_all(parent_dir).map_err(io_error)?;
    }

    let mut conn = Connection::open(index_path)?;
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    enable_wal_mode(&conn)?;

    operation(&mut conn)
}

fn enable_wal_mode(conn: &Connection) -> Result<(), SessionStoreError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let journal_mode: String = conn.query_row("PRAGMA journal_mode;", [], |row| row.get(0))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(SessionStoreError::CorruptIndex {
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

        CREATE TABLE IF NOT EXISTS message_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts INTEGER NOT NULL,
            text TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_entries (
            scope TEXT NOT NULL,
            reference_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            requested_order INTEGER,
            PRIMARY KEY (scope, reference_id)
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_extra_prompts (
            scope TEXT NOT NULL,
            reference_id TEXT NOT NULL,
            title TEXT NOT NULL,
            body TEXT NOT NULL,
            PRIMARY KEY (scope, reference_id)
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_core_overrides (
            scope TEXT PRIMARY KEY,
            body TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_skill_discovery_overrides (
            scope TEXT PRIMARY KEY,
            body TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_skill_discovery_skills (
            scope TEXT NOT NULL,
            skill_name TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            requested_order INTEGER,
            PRIMARY KEY (scope, skill_name)
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_tool_guideline_overrides (
            scope TEXT PRIMARY KEY,
            body TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_tool_selections (
            scope TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            requested_order INTEGER,
            PRIMARY KEY (scope, tool_name)
        );

        CREATE TABLE IF NOT EXISTS prompt_assembly_dynamic_environment_sources (
            scope TEXT NOT NULL,
            snapshot_kind TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            enabled INTEGER NOT NULL,
            PRIMARY KEY (scope, snapshot_kind, source_kind)
        );
        ",
    )
    .map_err(SessionStoreError::from)
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
            meta.project_dir.canonical_string(),
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
            meta.project_dir.as_path().to_string_lossy(),
            meta.jsonl_path.to_string_lossy(),
        ],
    )?;

    Ok(())
}

fn get_session_meta_row(
    conn: &Connection,
    session_id: &str,
) -> Result<SessionMeta, SessionStoreError> {
    let requested_session_id = session_id
        .parse()
        .map_err(|_| SessionStoreError::CorruptIndex {
            message: format!("session id `{session_id}` is not a valid UUIDv7"),
        })?;
    let meta = conn
        .query_row(
            "
            SELECT
                sessions.session_id,
                sessions.project_dir,
                sessions.title,
                sessions.preview,
                sessions.first_user_preview,
                sessions.last_assistant_preview,
                sessions.total_tokens,
                sessions.model,
                sessions.created_at,
                sessions.updated_at,
                sessions.git_head,
                sessions.work_dir,
                sessions.jsonl_path,
                session_repair_state.file_size
            FROM sessions
            LEFT JOIN session_repair_state
                ON session_repair_state.session_id = sessions.session_id
            WHERE sessions.session_id = ?1
            ",
            params![session_id],
            row_to_session_meta,
        )
        .optional()?;

    meta.ok_or(SessionStoreError::SessionNotFound {
        session_id: requested_session_id,
    })
}

fn list_session_rows(
    conn: &Connection,
    project_dir: &str,
) -> Result<Vec<SessionMeta>, SessionStoreError> {
    let mut statement = conn.prepare(
        "
            SELECT
                sessions.session_id,
                sessions.project_dir,
                sessions.title,
                sessions.preview,
                sessions.first_user_preview,
                sessions.last_assistant_preview,
                sessions.total_tokens,
                sessions.model,
                sessions.created_at,
                sessions.updated_at,
                sessions.git_head,
                sessions.work_dir,
                sessions.jsonl_path,
                session_repair_state.file_size
            FROM sessions
            LEFT JOIN session_repair_state
                ON session_repair_state.session_id = sessions.session_id
            WHERE sessions.project_dir = ?1
            ORDER BY updated_at DESC, created_at DESC, sessions.session_id DESC
            ",
    )?;
    let rows = statement.query_map(params![project_dir], row_to_session_meta)?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row?);
    }

    Ok(sessions)
}

fn checked_i64(value: u64, label: &str) -> Result<i64, SessionStoreError> {
    i64::try_from(value).map_err(|_| SessionStoreError::CorruptIndex {
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
        project_dir: ProjectDir::from_stored_path(PathBuf::from(row.get::<_, String>(11)?)),
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
        jsonl_path: PathBuf::from(row.get::<_, String>(12)?),
        size_bytes: row
            .get::<_, Option<i64>>(13)?
            .map(|size| {
                u64::try_from(size).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        13,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })
            })
            .transpose()?,
    })
}

fn io_error(source: std::io::Error) -> SessionStoreError {
    SessionStoreError::IoError { source }
}

fn is_sqlite_busy_error(error: &SessionStoreError) -> bool {
    matches!(
        error,
        SessionStoreError::SqliteError { source }
            if source.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
    )
}

#[cfg(test)]
mod tests;
