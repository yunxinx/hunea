use runtime_domain::session::should_record_message_history_text;

use crate::{AppEffect, Model};

/// 仅更新盲回溯缓存（发送路径在 model 层先执行，持久化由 runner 在 `SendConversationTurn` 时写入）。
pub(crate) fn stage_message_history_recall(model: &mut Model, text: String) {
    if should_record_message_history_text(&text) {
        model.blind_recall.push_local_entry(text);
    }
}

/// 写入盲回溯并返回异步持久化 effect（清空输入、picker Enter 等路径）。
pub(crate) fn commit_message_history(model: &mut Model, text: String) -> Option<AppEffect> {
    if !should_record_message_history_text(&text) {
        return None;
    }
    model.blind_recall.push_local_entry(text.clone());
    Some(AppEffect::RecordMessageHistory { text })
}

pub(crate) fn message_history_record_effect(text: String) -> Option<AppEffect> {
    should_record_message_history_text(&text).then_some(AppEffect::RecordMessageHistory { text })
}
