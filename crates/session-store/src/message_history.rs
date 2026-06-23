//! 全局 message history 持久化（`index.sqlite` 的 `message_history` 表）。

use runtime_domain::session::{MessageHistoryEntry, MessageHistoryRow};
use rusqlite::{Connection, OptionalExtension, params};

use crate::SessionStoreError;

/// 盲回溯启动缓存固定条数（与 PRD 一致）。
pub const MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN: usize = 25;

pub(crate) fn record_message_history(
    index_path: &std::path::Path,
    text: &str,
    limit: usize,
) -> Result<(), SessionStoreError> {
    if text.is_empty() {
        return Ok(());
    }

    crate::metadata::with_connection(index_path, |conn| {
        let last_text: Option<String> = conn
            .query_row(
                "SELECT text FROM message_history ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;

        if last_text.as_deref() == Some(text) {
            return Ok(());
        }

        let ts = crate::store::current_timestamp_ms()?;
        conn.execute(
            "INSERT INTO message_history (ts, text) VALUES (?1, ?2)",
            params![ts, text],
        )
        .map_err(sqlite_err)?;

        trim_message_history(conn, limit)?;
        Ok(())
    })
}

pub(crate) fn load_message_history_recent(
    index_path: &std::path::Path,
    limit: usize,
) -> Result<Vec<MessageHistoryEntry>, SessionStoreError> {
    crate::metadata::with_connection(index_path, |conn| {
        let mut statement = conn
            .prepare("SELECT ts, text FROM message_history ORDER BY id DESC LIMIT ?1")
            .map_err(sqlite_err)?;
        let limit_param = i64::try_from(limit).map_err(|_| SessionStoreError::CorruptIndex {
            message: "message_history recent limit exceeds sqlite INTEGER range".to_string(),
        })?;
        let mut entries = statement
            .query_map(params![limit_param], |row| {
                Ok(MessageHistoryEntry {
                    ts: row.get(0)?,
                    text: row.get(1)?,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)?;
        entries.reverse();
        Ok(entries)
    })
}

pub(crate) fn load_message_history_all(
    index_path: &std::path::Path,
) -> Result<Vec<MessageHistoryRow>, SessionStoreError> {
    crate::metadata::with_connection(index_path, |conn| {
        let mut statement = conn
            .prepare("SELECT id, ts, text FROM message_history ORDER BY id ASC")
            .map_err(sqlite_err)?;
        statement
            .query_map([], |row| {
                Ok(MessageHistoryRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    text: row.get(2)?,
                })
            })
            .map_err(sqlite_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    })
}

fn sqlite_err(source: rusqlite::Error) -> SessionStoreError {
    SessionStoreError::SqliteError { source }
}

fn trim_message_history(conn: &Connection, limit: usize) -> Result<(), SessionStoreError> {
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM message_history", [], |row| row.get(0))
        .map_err(sqlite_err)?;
    let limit_i64 = i64::try_from(limit).map_err(|_| SessionStoreError::CorruptIndex {
        message: "message_history_limit exceeds sqlite INTEGER range".to_string(),
    })?;
    let excess = count.saturating_sub(limit_i64);
    if excess > 0 {
        conn.execute(
            "DELETE FROM message_history WHERE id IN (
                SELECT id FROM message_history ORDER BY id ASC LIMIT ?1
            )",
            params![excess],
        )
        .map_err(sqlite_err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
