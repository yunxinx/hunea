use crate::{AppEffect, Model};

/// 更新盲回溯缓存；若写入新条目则返回待持久化正文（由 `SendConversationTurn` 携带，避免再从 request 抽取）。
pub(crate) fn stage_message_history_recall(
    model: &mut Model,
    text: &str,
) -> Option<runtime_domain::session::PendingMessageHistoryEntry> {
    model.blind_recall.push_local_entry(text)
}

/// 写入盲回溯并返回异步持久化 effect（清空输入、picker Enter 等路径）。
pub(crate) fn commit_message_history(model: &mut Model, text: &str) -> Option<AppEffect> {
    let entry = model.blind_recall.push_local_entry(text)?;
    Some(AppEffect::RecordMessageHistory {
        entry_id: entry.id,
        text: entry.text,
    })
}
