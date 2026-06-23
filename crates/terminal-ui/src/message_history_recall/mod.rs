//! 盲回溯（空输入或仍匹配上次 recall 时的 Up/Down）状态机。

mod state;

#[cfg(test)]
mod tests;

use session_store::MessageHistoryEntry;
pub(crate) use state::{BlindRecallNavigateResult, BlindRecallState};

/// 将 SQLite 近期查询结果转为 oldest-first 盲回溯缓存（最多 25 条）。
pub(crate) fn startup_cache_from_recent(
    entries: Vec<MessageHistoryEntry>,
) -> Vec<MessageHistoryEntry> {
    let mut cache = entries;
    cache.reverse();
    cache
}
