use runtime_domain::session::MessageHistoryEntry;
use session_store::MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN;

use crate::time::current_unix_timestamp_ms;

/// 固定 25 条、oldest-first 的 shell 风格 history 状态机（无 async fetch）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BlindRecallState {
    cache: Vec<MessageHistoryEntry>,
    history_cursor: Option<usize>,
    last_history_text: Option<String>,
}

impl BlindRecallState {
    pub(crate) fn replace_cache(&mut self, entries: Vec<MessageHistoryEntry>) {
        self.cache = entries;
        self.history_cursor = None;
        self.last_history_text = None;
    }

    #[cfg(test)]
    pub(crate) fn cache(&self) -> &[MessageHistoryEntry] {
        &self.cache
    }

    #[cfg(test)]
    pub(crate) fn history_cursor(&self) -> Option<usize> {
        self.history_cursor
    }

    #[cfg(test)]
    pub(crate) fn last_history_text(&self) -> Option<&str> {
        self.last_history_text.as_deref()
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
        matches!(&self.last_history_text, Some(prev) if prev == text)
    }

    /// 上一条 history；成功时写入 `last_history_text`，调用方用 [`Self::active_history_text`] 取正文。
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
        self.last_history_text = Some(self.cache[next_idx].text.clone());
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
                self.last_history_text = None;
                return Some(false);
            }
            Some(idx) => idx + 1,
        };

        self.history_cursor = Some(next);
        self.last_history_text = Some(self.cache[next].text.clone());
        Some(true)
    }

    /// 最近一次导航或 recall 后的 history 正文（清空 composer 时为 `None`）。
    pub(crate) fn active_history_text(&self) -> Option<&str> {
        self.last_history_text.as_deref()
    }

    /// 本地写入（发送 / Ctrl-C 清输入）：相邻去重、trim 至 25、重置导航。
    pub(crate) fn push_local_entry(&mut self, text: String) {
        if !runtime_domain::session::should_record_message_history_text(&text) {
            return;
        }
        self.history_cursor = None;
        self.last_history_text = None;

        if self.cache.last().is_some_and(|prev| prev.text == text) {
            return;
        }

        let ts = current_unix_timestamp_ms();
        self.cache.push(MessageHistoryEntry { ts, text });

        if self.cache.len() > MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN {
            let overflow = self.cache.len() - MESSAGE_HISTORY_BLIND_RECALL_CACHE_LEN;
            self.cache.drain(0..overflow);
            self.history_cursor = None;
        }
    }

    /// Picker Enter 恢复全文后，与盲回溯 Up 填入条目一致的门控状态。
    pub(crate) fn apply_recalled_text(&mut self, text: &str) {
        self.history_cursor = self.cache.iter().rposition(|entry| entry.text == text);
        self.last_history_text = Some(text.to_string());
    }
}
