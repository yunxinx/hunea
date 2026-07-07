//! 全局 message history 持久化（`index.sqlite` 的 `message_history` 表）。

use runtime_domain::session::{
    MessageHistoryEntry, MessageHistoryRow, message_history_is_adjacent_duplicate,
    should_record_message_history_text,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::SessionStoreError;

pub(crate) fn record_message_history(
    index_path: &std::path::Path,
    text: &str,
    limit: usize,
) -> Result<(), SessionStoreError> {
    if !should_record_message_history_text(text) {
        return Ok(());
    }

    crate::metadata::with_connection(index_path, |conn| {
        let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let last_text: Option<String> = transaction
            .query_row(
                "SELECT text FROM message_history ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;

        if message_history_is_adjacent_duplicate(last_text.as_deref(), text) {
            transaction.commit()?;
            return Ok(());
        }

        let ts = crate::store::current_timestamp_ms()?;
        transaction.execute(
            "INSERT INTO message_history (ts, text) VALUES (?1, ?2)",
            params![ts, text],
        )?;

        trim_message_history(&transaction, limit)?;
        transaction.commit()?;
        Ok(())
    })
}

pub(crate) fn load_message_history_recent(
    index_path: &std::path::Path,
    limit: usize,
) -> Result<Vec<MessageHistoryEntry>, SessionStoreError> {
    crate::metadata::with_connection(index_path, |conn| {
        let mut statement =
            conn.prepare("SELECT ts, text FROM message_history ORDER BY id DESC LIMIT ?1")?;
        let limit_param = i64::try_from(limit).map_err(|_| SessionStoreError::CorruptIndex {
            message: "message_history recent limit exceeds sqlite INTEGER range".to_string(),
        })?;
        let mut entries = statement
            .query_map(params![limit_param], |row| {
                Ok(MessageHistoryEntry {
                    ts: row.get(0)?,
                    text: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        entries.reverse();
        Ok(entries)
    })
}

pub(crate) fn load_message_history_all(
    index_path: &std::path::Path,
) -> Result<Vec<MessageHistoryRow>, SessionStoreError> {
    crate::metadata::with_connection(index_path, |conn| {
        let mut statement =
            conn.prepare("SELECT id, ts, text FROM message_history ORDER BY id ASC")?;
        statement
            .query_map([], |row| {
                Ok(MessageHistoryRow {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    text: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(SessionStoreError::from)
    })
}

fn trim_message_history(conn: &Connection, limit: usize) -> Result<(), SessionStoreError> {
    let limit = i64::try_from(limit).map_err(|_| SessionStoreError::CorruptIndex {
        message: "message_history limit exceeds sqlite INTEGER range".to_string(),
    })?;
    conn.execute(
        "DELETE FROM message_history
         WHERE id NOT IN (
             SELECT id FROM message_history ORDER BY id DESC LIMIT ?1
        )",
        params![limit],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
