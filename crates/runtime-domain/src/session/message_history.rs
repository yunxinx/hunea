//! Message history 在 runtime 事件与命令边界上的载荷类型（持久化由 session-store 负责）。

/// 盲回溯启动缓存固定条数。
pub const MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN: usize = 25;

/// 盲回溯启动缓存单条记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHistoryEntry {
    pub ts: i64,
    pub text: String,
}

/// 全屏 history picker 列表行（含稳定 id）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageHistoryRow {
    pub id: i64,
    pub ts: i64,
    pub text: String,
}

/// 是否应写入 message history（与发送门控一致：纯空白等无意义内容不入库）。
pub fn should_record_message_history_text(text: &str) -> bool {
    !text.trim().is_empty()
}

/// 判断待写入正文是否与上一条 message history 相邻重复。
pub fn message_history_is_adjacent_duplicate(previous_text: Option<&str>, text: &str) -> bool {
    previous_text == Some(text)
}

/// 计算当前条数超过上限时应丢弃的最旧条数。
pub fn message_history_trim_excess_count(current_len: usize, limit: usize) -> usize {
    current_len.saturating_sub(limit)
}

/// 将缓存裁剪到指定上限，保留最新条目。
pub fn trim_message_history_entries(entries: &mut Vec<MessageHistoryEntry>, limit: usize) {
    let excess = message_history_trim_excess_count(entries.len(), limit);
    if excess > 0 {
        entries.drain(0..excess);
    }
}

/// 按统一策略追加一条 message history：相邻同文整条 no-op（不插入、不裁剪）；否则追加并在超限时裁掉最旧条目。
pub fn append_message_history_entry(
    entries: &mut Vec<MessageHistoryEntry>,
    entry: MessageHistoryEntry,
    limit: usize,
) {
    if message_history_is_adjacent_duplicate(
        entries.last().map(|previous| previous.text.as_str()),
        &entry.text,
    ) {
        return;
    }

    entries.push(entry);
    trim_message_history_entries(entries, limit);
}

/// 合并两组 oldest-first 条目，并应用同一套相邻去重与上限裁剪策略。
pub fn merge_message_history_entries(
    persisted_entries: Vec<MessageHistoryEntry>,
    local_entries: Vec<MessageHistoryEntry>,
    limit: usize,
) -> Vec<MessageHistoryEntry> {
    let mut merged = Vec::with_capacity(persisted_entries.len() + local_entries.len());
    for entry in persisted_entries.into_iter().chain(local_entries) {
        append_message_history_entry(&mut merged, entry, limit);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::{
        MessageHistoryEntry, append_message_history_entry, merge_message_history_entries,
        message_history_trim_excess_count, should_record_message_history_text,
    };

    #[test]
    fn skips_empty_and_whitespace_only() {
        assert!(!should_record_message_history_text(""));
        assert!(!should_record_message_history_text("   "));
        assert!(!should_record_message_history_text("\t\n"));
        assert!(should_record_message_history_text("x"));
        assert!(should_record_message_history_text("  hi  "));
    }

    #[test]
    fn adjacent_duplicate_is_noop_without_trim() {
        let mut entries = vec![entry(1, "a"), entry(2, "b"), entry(3, "c")];

        append_message_history_entry(&mut entries, entry(4, "c"), 3);
        assert_eq!(texts(&entries), ["a", "b", "c"]);
    }

    #[test]
    fn appends_with_adjacent_dedup_and_trim_on_new_entry() {
        let mut entries = vec![
            entry(1, "older"),
            entry(2, "same"),
            entry(3, "same"),
            entry(4, "newer"),
        ];

        append_message_history_entry(&mut entries, entry(5, "newer"), 3);
        assert_eq!(texts(&entries), ["older", "same", "same", "newer"]);

        append_message_history_entry(&mut entries, entry(6, "fresh"), 3);
        assert_eq!(texts(&entries), ["same", "newer", "fresh"]);
    }

    #[test]
    fn merges_sources_with_one_adjacent_dedup_and_trim_rule() {
        let merged = merge_message_history_entries(
            vec![entry(1, "persisted"), entry(2, "same")],
            vec![entry(3, "same"), entry(4, "local")],
            3,
        );

        assert_eq!(texts(&merged), ["persisted", "same", "local"]);
    }

    #[test]
    fn trim_excess_count_saturates_at_limit() {
        assert_eq!(message_history_trim_excess_count(3, 5), 0);
        assert_eq!(message_history_trim_excess_count(5, 5), 0);
        assert_eq!(message_history_trim_excess_count(8, 5), 3);
    }

    fn entry(ts: i64, text: &str) -> MessageHistoryEntry {
        MessageHistoryEntry {
            ts,
            text: text.to_string(),
        }
    }

    fn texts(entries: &[MessageHistoryEntry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.text.as_str()).collect()
    }
}
