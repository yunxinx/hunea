use std::collections::BTreeMap;
use std::time::Instant;

use runtime_domain::model_catalog::ModelSelection;

use crate::{
    document::{
        LayoutCache, RestoreState, SmoothScrollState, StableTailLayoutCache, TailLayoutCache,
        TranscriptCache, ViewportCache, ViewportState,
    },
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
};

/// `SelectedModelState` 收口当前模型选择及其缓存失效 revision。
///
/// 唯一写路径是 [`SelectedModelState::set`]，保证任何选择变化都伴随 revision 递增，
/// 使 tail layout cache key 只比较 revision，不必每帧分配并比较 display name。
#[derive(Debug, Clone, Default)]
pub(crate) struct SelectedModelState {
    selection: Option<ModelSelection>,
    revision: usize,
}

impl SelectedModelState {
    pub(crate) fn new(selection: Option<ModelSelection>) -> Self {
        Self {
            selection,
            revision: 0,
        }
    }

    pub(crate) fn selection(&self) -> Option<&ModelSelection> {
        self.selection.as_ref()
    }

    pub(crate) fn set(&mut self, selection: Option<ModelSelection>) {
        if self.selection == selection {
            return;
        }
        self.selection = selection;
        self.revision = self.revision.saturating_add(1);
    }

    pub(crate) fn revision(&self) -> usize {
        self.revision
    }
}

#[cfg(test)]
mod tests {
    use runtime_domain::model_catalog::ModelSelection;

    use super::SelectedModelState;

    #[test]
    fn selected_model_revision_changes_only_when_selection_changes() {
        let first_selection = ModelSelection::new("local", "qwen3");
        let second_selection = ModelSelection::new("remote", "gpt-5");
        let mut state = SelectedModelState::new(Some(first_selection.clone()));

        assert_eq!(state.revision(), 0);

        state.set(Some(first_selection));
        assert_eq!(
            state.revision(),
            0,
            "equal selection must keep the cache key"
        );

        state.set(Some(second_selection));
        assert_eq!(
            state.revision(),
            1,
            "changed selection must invalidate caches"
        );

        state.set(None);
        assert_eq!(
            state.revision(),
            2,
            "clearing selection must invalidate caches"
        );
    }
}

/// `SelectionRuntimeState` 收口 selection 与拖拽自动滚动的运行态。
#[derive(Debug, Clone)]
pub(crate) struct SelectionRuntimeState {
    pub(crate) selection: SelectionState,
    pub(crate) click: SelectionClickState,
    pub(crate) version: usize,
    pub(crate) auto_scroll_direction: AutoScrollDirection,
    pub(crate) auto_scroll_token: usize,
    pub(crate) auto_scroll_mouse: MousePosition,
    pub(crate) auto_scroll_deadline: Option<Instant>,
}

impl Default for SelectionRuntimeState {
    fn default() -> Self {
        Self {
            selection: SelectionState::default(),
            click: SelectionClickState::default(),
            version: 0,
            auto_scroll_direction: AutoScrollDirection::None,
            auto_scroll_token: 0,
            auto_scroll_mouse: MousePosition::default(),
            auto_scroll_deadline: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PendingReasoningToggleClick {
    pub(crate) item_index: usize,
    pub(crate) column: u16,
    pub(crate) row: u16,
    pub(crate) active: bool,
}

/// `DocumentRuntimeState` 收口统一文档 viewport、cache 与手动滚动状态。
#[derive(Debug, Clone, Default)]
pub(crate) struct DocumentRuntimeState {
    pub(crate) viewport_y: usize,
    pub(crate) viewport_state: ViewportState,
    pub(crate) transcript_cache: TranscriptCache,
    pub(crate) stable_tail_layout_cache: StableTailLayoutCache,
    pub(crate) tail_layout_cache: TailLayoutCache,
    pub(crate) layout_cache: LayoutCache,
    pub(crate) viewport_cache: ViewportCache,
    pub(crate) follow_bottom: bool,
    pub(crate) manual_scroll: bool,
    pub(crate) restore: RestoreState,
    /// 滚轮平滑滚动累加器；纯瞬态，不进 `ViewportState` 语义锚点。
    pub(crate) smooth_scroll: SmoothScrollState,
}

/// `NoticeState` 收口底部状态行上的短暂提示、滚动提示、外部编辑器提示与退出确认。
///
/// 状态行提示用于不会打断阅读节奏的导航与确认类反馈，例如退出确认与 Esc 中断提示。
/// 需要醒目确认的结果性事件应使用上层 `ToastState`，避免占用底部状态槽并移动文档内容。
#[derive(Debug, Clone, Default)]
pub(crate) struct NoticeState {
    pub(crate) status_text: String,
    pub(crate) status_token: usize,
    pub(crate) status_deadline: Option<Instant>,
    pub(crate) history_scroll_indicator_token: usize,
    pub(crate) history_scroll_indicator_deadline: Option<Instant>,
    pub(crate) external_editor_helper_visible: bool,
    pub(crate) external_editor_helper_token: usize,
    pub(crate) external_editor_helper_deadline: Option<Instant>,
    pub(crate) exit_confirmation_deadline: Option<Instant>,
}

/// `AgentSettledExpiryState` 收口 settled child 自动销毁的唤醒登记。
///
/// 登记由 `AgentOutcomeFact` 驱动（按 `occurred_at_ms` 换算剩余窗口）；到点只负责
/// 唤醒 loop 迭代，销毁本身由 runtime drain 中的过期清扫执行，TUI 不直接派发删除。
/// child 行移除（Remove delta）、会话切换或 runtime 失效时清除登记；到点即消费，
/// 防止清扫未收敛（CleanupBlocked）时过期 deadline 让事件泵空转。
#[derive(Debug, Clone, Default)]
pub(crate) struct AgentSettledExpiryState {
    deadlines: BTreeMap<runtime_domain::agent::AgentId, Instant>,
}

impl AgentSettledExpiryState {
    pub(crate) fn register(&mut self, agent_id: runtime_domain::agent::AgentId, deadline: Instant) {
        self.deadlines.insert(agent_id, deadline);
    }

    pub(crate) fn remove(&mut self, agent_id: runtime_domain::agent::AgentId) {
        self.deadlines.remove(&agent_id);
    }

    pub(crate) fn clear(&mut self) {
        self.deadlines.clear();
    }

    /// 最早到期的销毁唤醒时刻；空登记返回 `None`。
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.deadlines.values().copied().min()
    }

    /// 消费已到期的登记。唤醒发生后销毁由下一次 runtime drain 兜底，这里只清登记。
    pub(crate) fn consume_expired(&mut self, now: Instant) {
        self.deadlines.retain(|_, deadline| *deadline > now);
    }

    #[cfg(test)]
    pub(crate) fn deadline(&self, agent_id: runtime_domain::agent::AgentId) -> Option<Instant> {
        self.deadlines.get(&agent_id).copied()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.deadlines.is_empty()
    }
}
