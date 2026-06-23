//! Message history 在 runtime 事件与命令边界上的载荷类型（持久化由 session-store 负责）。

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

#[cfg(test)]
mod tests {
    use super::should_record_message_history_text;

    #[test]
    fn skips_empty_and_whitespace_only() {
        assert!(!should_record_message_history_text(""));
        assert!(!should_record_message_history_text("   "));
        assert!(!should_record_message_history_text("\t\n"));
        assert!(should_record_message_history_text("x"));
        assert!(should_record_message_history_text("  hi  "));
    }
}
