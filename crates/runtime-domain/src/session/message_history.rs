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
