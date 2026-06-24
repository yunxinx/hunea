use runtime_domain::{
    session::{
        MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN, MessageHistoryEntry, MessageHistoryEntryId,
        PendingMessageHistoryEntry, append_message_history_entry, merge_message_history_entries,
        message_history_is_adjacent_duplicate, should_record_message_history_text,
    },
    time::unix_timestamp_ms,
};

/// 固定 25 条、oldest-first 的 shell 风格 history 状态机（无 async fetch）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BlindRecallState {
    persisted_cache: Vec<MessageHistoryEntry>,
    pending_entries: Vec<PendingMessageHistoryEntry>,
    next_entry_id: u64,
    cache: Vec<MessageHistoryEntry>,
    history_cursor: Option<usize>,
    active_recall: Option<BlindRecallAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BlindRecallAnchor {
    CacheIndex(usize),
    ExternalText(String),
}

impl BlindRecallState {
    pub(crate) fn apply_startup_cache(&mut self, entries: Vec<MessageHistoryEntry>) {
        self.persisted_cache = merge_message_history_entries(
            entries,
            Vec::new(),
            MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
        );
        self.rebuild_cache();
        self.reset_navigation();
    }

    #[cfg(test)]
    pub(crate) fn replace_cache(&mut self, entries: Vec<MessageHistoryEntry>) {
        self.persisted_cache = merge_message_history_entries(
            entries,
            Vec::new(),
            MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
        );
        self.pending_entries.clear();
        self.next_entry_id = 0;
        self.rebuild_cache();
        self.reset_navigation();
    }

    #[cfg(test)]
    pub(crate) fn cache(&self) -> Vec<MessageHistoryEntry> {
        self.cache.clone()
    }

    #[cfg(test)]
    pub(crate) fn pending_entry_id_for_test(&self) -> Option<MessageHistoryEntryId> {
        self.pending_entries.last().map(|entry| entry.id)
    }

    #[cfg(test)]
    pub(crate) fn history_cursor(&self) -> Option<usize> {
        self.history_cursor
    }

    #[cfg(test)]
    pub(crate) fn last_history_text(&self) -> Option<&str> {
        self.active_history_text()
    }

    /// 是否应由 Up/Down 走 history 而非 composer 行内移动。
    pub(crate) fn should_handle_navigation(&self, text: &str, cursor: usize) -> bool {
        if self.cache.is_empty() {
            return false;
        }
        if text.is_empty() {
            return true;
        }
        // composer 光标是字符索引，不是 UTF-8 字节偏移。
        let char_len = text.chars().count();
        if cursor != 0 && cursor != char_len {
            return false;
        }
        self.active_history_text() == Some(text)
    }

    /// 上一条 history；成功时只保存缓存索引，调用方用 [`Self::active_history_text`] 取正文。
    pub(crate) fn navigate_up(&mut self) -> bool {
        let len = self.cache.len();
        if len == 0 {
            return false;
        }

        let next_idx = match self.history_cursor {
            None => len - 1,
            Some(0) => return false,
            Some(idx) => idx - 1,
        };

        self.history_cursor = Some(next_idx);
        self.active_recall = Some(BlindRecallAnchor::CacheIndex(next_idx));
        true
    }

    /// 下一条 history；`Some(true)` 为条目正文，`Some(false)` 为越过最新后清空 composer。
    pub(crate) fn navigate_down(&mut self) -> Option<bool> {
        let len = self.cache.len();
        if len == 0 {
            return None;
        }

        let next = match self.history_cursor {
            None => return None,
            Some(idx) if idx + 1 >= len => {
                self.history_cursor = None;
                self.active_recall = None;
                return Some(false);
            }
            Some(idx) => idx + 1,
        };

        self.history_cursor = Some(next);
        self.active_recall = Some(BlindRecallAnchor::CacheIndex(next));
        Some(true)
    }

    /// 最近一次导航或 recall 后的 history 正文（清空 composer 时为 `None`）。
    pub(crate) fn active_history_text(&self) -> Option<&str> {
        match self.active_recall.as_ref()? {
            BlindRecallAnchor::CacheIndex(index) => {
                self.cache.get(*index).map(|entry| entry.text.as_str())
            }
            BlindRecallAnchor::ExternalText(text) => Some(text.as_str()),
        }
    }

    /// 本地写入（发送 / Ctrl-C 清输入）：相邻同文 no-op，否则追加并 trim 至 25，重置导航。
    ///
    /// 若实际写入了新条目，返回应用层应持久化的待提交条目。
    pub(crate) fn push_local_entry(&mut self, text: &str) -> Option<PendingMessageHistoryEntry> {
        self.push_local_entry_with_timestamp(text, unix_timestamp_ms().ok())
    }

    #[cfg(test)]
    pub(crate) fn push_local_entry_with_timestamp_for_test(
        &mut self,
        text: &str,
        timestamp_ms: Option<i64>,
    ) -> Option<PendingMessageHistoryEntry> {
        self.push_local_entry_with_timestamp(text, timestamp_ms)
    }

    fn push_local_entry_with_timestamp(
        &mut self,
        text: &str,
        timestamp_ms: Option<i64>,
    ) -> Option<PendingMessageHistoryEntry> {
        if !should_record_message_history_text(text) {
            return None;
        }
        self.reset_navigation();

        if message_history_is_adjacent_duplicate(
            self.cache.last().map(|previous| previous.text.as_str()),
            text,
        ) {
            return None;
        }

        let pending_entry = PendingMessageHistoryEntry {
            id: self.allocate_entry_id(),
            ts: timestamp_ms?,
            text: text.to_string(),
        };
        self.pending_entries.push(pending_entry.clone());
        self.rebuild_cache();
        Some(pending_entry)
    }

    /// 记录异步持久化成功，将该条目从待提交队列晋升到已持久化缓存。
    pub(crate) fn confirm_persisted(&mut self, entry_id: MessageHistoryEntryId) -> bool {
        let Some(index) = self
            .pending_entries
            .iter()
            .position(|entry| entry.id == entry_id)
        else {
            return false;
        };
        let persisted = self.pending_entries.remove(index);
        append_message_history_entry(
            &mut self.persisted_cache,
            persisted.as_history_entry(),
            MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
        );
        self.rebuild_cache();
        true
    }

    /// 异步持久化失败时回滚对应的待提交条目，并根据剩余 pending/persisted 状态重建可见缓存。
    pub(crate) fn revert_failed_persist(&mut self, entry_id: MessageHistoryEntryId) -> bool {
        let Some(index) = self
            .pending_entries
            .iter()
            .position(|entry| entry.id == entry_id)
        else {
            return false;
        };
        self.pending_entries.remove(index);
        self.rebuild_cache();
        self.reset_navigation();
        true
    }

    /// Picker Enter 恢复全文后，与盲回溯 Up 填入条目一致的门控状态。
    pub(crate) fn apply_recalled_text(&mut self, text: &str) {
        self.history_cursor = self.cache.iter().rposition(|entry| entry.text == text);
        self.active_recall = Some(match self.history_cursor {
            Some(index) => BlindRecallAnchor::CacheIndex(index),
            None => BlindRecallAnchor::ExternalText(text.to_string()),
        });
    }

    fn rebuild_cache(&mut self) {
        self.cache = merged_message_history_cache(
            self.persisted_cache.clone(),
            self.pending_entries
                .iter()
                .map(PendingMessageHistoryEntry::as_history_entry)
                .collect(),
        );
    }

    fn allocate_entry_id(&mut self) -> MessageHistoryEntryId {
        self.next_entry_id = self.next_entry_id.saturating_add(1).max(1);
        MessageHistoryEntryId(self.next_entry_id)
    }

    fn reset_navigation(&mut self) {
        self.history_cursor = None;
        self.active_recall = None;
    }
}

fn merged_message_history_cache(
    persisted_entries: Vec<MessageHistoryEntry>,
    local_entries: Vec<MessageHistoryEntry>,
) -> Vec<MessageHistoryEntry> {
    merge_message_history_entries(
        persisted_entries,
        local_entries,
        MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
    )
}
