use runtime_domain::{
    session::{
        MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN, MessageHistoryEntry, append_message_history_entry,
        merge_message_history_entries, message_history_is_adjacent_duplicate,
        revert_message_history_tail_entry, should_record_message_history_text,
    },
    time::unix_timestamp_ms,
};

/// 固定 25 条、oldest-first 的 shell 风格 history 状态机（无 async fetch）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BlindRecallState {
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
        if self.cache.is_empty() {
            self.replace_cache(entries);
            return;
        }

        let local_entries = std::mem::take(&mut self.cache);
        self.cache = merged_message_history_cache(entries, local_entries);
        self.history_cursor = None;
        self.active_recall = None;
    }

    pub(crate) fn replace_cache(&mut self, entries: Vec<MessageHistoryEntry>) {
        self.cache = merge_message_history_entries(
            entries,
            Vec::new(),
            MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
        );
        self.history_cursor = None;
        self.active_recall = None;
    }

    #[cfg(test)]
    pub(crate) fn cache(&self) -> &[MessageHistoryEntry] {
        &self.cache
    }

    /// 盲回溯缓存末尾正文（持久化调度失败时用于回滚键）。
    pub(crate) fn tail_entry_text_for_persist_revert(&self) -> Option<&str> {
        self.cache.last().map(|entry| entry.text.as_str())
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
        if cursor != 0 && cursor != text.len() {
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
    /// 若实际写入了新条目，返回应用层应持久化的正文（与缓存末尾条目内容一致）。
    pub(crate) fn push_local_entry(&mut self, text: &str) -> Option<String> {
        if !should_record_message_history_text(text) {
            return None;
        }
        self.history_cursor = None;
        self.active_recall = None;

        if message_history_is_adjacent_duplicate(
            self.cache.last().map(|previous| previous.text.as_str()),
            text,
        ) {
            return None;
        }

        let ts = unix_timestamp_ms().unwrap_or(0);
        let persisted_text = text.to_string();
        append_message_history_entry(
            &mut self.cache,
            MessageHistoryEntry {
                ts,
                text: persisted_text.clone(),
            },
            MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN,
        );
        Some(persisted_text)
    }

    /// 异步持久化失败时回滚盲回溯缓存末尾一条（与 [`push_local_entry`] 写入的正文一致时）。
    pub(crate) fn revert_failed_persist(&mut self, text: &str) -> bool {
        if !revert_message_history_tail_entry(&mut self.cache, text) {
            return false;
        }
        self.history_cursor = None;
        self.active_recall = None;
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
