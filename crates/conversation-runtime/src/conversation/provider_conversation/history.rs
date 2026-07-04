use provider_protocol::{ConversationItem, Role};
use session_store::SessionId;

use super::{ProviderConversation, ProviderConversationError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedConversationItem {
    pub entry_id: Option<String>,
    pub item: ConversationItem,
}

impl ProviderConversation {
    /// `truncate_after_user_turns` 保留指定数量的已提交 user turns。
    #[must_use = "truncating provider history may require a persisted leaf update"]
    pub fn truncate_after_user_turns(
        &mut self,
        retained_user_turns: usize,
    ) -> Result<Option<(SessionId, String)>, ProviderConversationError> {
        self.pending_user_items.clear();
        self.pending_dynamic_environment_observations = None;
        self.upstream_context_tokens = None;
        let mut user_turn_count = 0usize;
        let mut truncate_index = self.persisted_history.len();
        for (index, item) in self.persisted_history.iter().enumerate() {
            if item.item.role() != Some(Role::User) {
                continue;
            }
            user_turn_count = user_turn_count.saturating_add(1);
            if user_turn_count > retained_user_turns {
                truncate_index = index;
                break;
            }
        }
        self.persisted_history.truncate(truncate_index);

        let leaf_update = if let Some(persistence) = self.persistence.as_ref()
            && let Some(session_id) = persistence.session_id.as_ref()
        {
            let leaf_id = self
                .persisted_history
                .last()
                .and_then(|item| item.entry_id.as_deref())
                .unwrap_or("header")
                .to_string();
            Some((session_id.clone(), leaf_id))
        } else {
            None
        };
        Ok(leaf_update)
    }

    /// `history` 以零拷贝方式返回当前 provider-visible 历史。
    #[must_use]
    pub fn history(&self) -> impl ExactSizeIterator<Item = &ConversationItem> + '_ {
        self.persisted_history
            .iter()
            .map(|persisted_item| &persisted_item.item)
    }

    /// `is_history_empty` 返回当前 provider-visible 历史是否为空。
    #[must_use]
    pub fn is_history_empty(&self) -> bool {
        self.persisted_history.is_empty()
    }

    /// `history_len` 返回当前 provider-visible 历史项数量。
    #[must_use]
    pub fn history_len(&self) -> usize {
        self.persisted_history.len()
    }

    /// `commit_pending_user` 把已开始发送给 provider 的当前用户消息写回会话历史。
    #[must_use]
    pub fn commit_pending_user(
        &mut self,
        entry_id: Option<String>,
        session_id: Option<SessionId>,
    ) -> bool {
        if let Some(session_id) = session_id {
            self.set_session_id(session_id);
        }
        if self.pending_user_items.is_empty() {
            return false;
        }
        let mut pending_items = std::mem::take(&mut self.pending_user_items);
        let last_index = pending_items.len().saturating_sub(1);
        for (index, item) in pending_items.drain(..).enumerate() {
            self.persisted_history.push(PersistedConversationItem {
                entry_id: (index == last_index).then(|| entry_id.clone()).flatten(),
                item,
            });
        }
        if let Some(observations) = self.pending_dynamic_environment_observations.take() {
            self.dynamic_environment_observations = observations;
        }
        true
    }

    /// `rollback_pending_user` 丢弃尚未开始发送给 provider 的当前用户消息。
    #[must_use]
    pub fn rollback_pending_user(&mut self) -> bool {
        let had_pending = !self.pending_user_items.is_empty();
        if had_pending {
            self.pending_user_items.clear();
            self.pending_dynamic_environment_observations = None;
            self.upstream_context_tokens = None;
        }
        had_pending
    }

    /// `commit_turn_items` 把 runtime 生成的 provider-visible 对话项写回会话历史。
    pub fn commit_turn_items(
        &mut self,
        items: impl IntoIterator<Item = PersistedConversationItem>,
    ) {
        let mut committed_any = false;
        for item in items {
            self.persisted_history.push(item);
            committed_any = true;
        }
        if committed_any {
            self.upstream_context_tokens = None;
        }
    }

    /// `append_items` 追加 provider-visible 对话项。
    #[must_use = "appending provider history can fail and must be handled"]
    pub fn append_items(
        &mut self,
        items: Vec<ConversationItem>,
    ) -> Result<(), ProviderConversationError> {
        if items.is_empty() {
            return Ok(());
        }
        self.upstream_context_tokens = None;

        self.commit_turn_items(items.into_iter().map(|item| PersistedConversationItem {
            entry_id: None,
            item,
        }));
        Ok(())
    }

    fn provider_items(&self) -> Vec<ConversationItem> {
        let mut items = Vec::with_capacity(
            self.persisted_history.len() + usize::from(self.system_prompt.is_some()),
        );
        if let Some(system_prompt) = self.system_prompt.as_deref() {
            items.push(ConversationItem::text(Role::System, system_prompt));
        }
        items.extend(self.persisted_history.iter().map(|item| item.item.clone()));
        items
    }

    pub(super) fn provider_items_with_pending_user_items(
        &self,
        user_items: &[ConversationItem],
    ) -> Vec<ConversationItem> {
        let mut items = self.provider_items();
        items.extend(user_items.iter().cloned());
        items
    }
}
