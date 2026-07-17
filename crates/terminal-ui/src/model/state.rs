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
