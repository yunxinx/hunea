use std::time::Instant;

use crate::{
    document::{
        LayoutCache, RestoreState, TailLayoutCache, TranscriptCache, ViewportCache, ViewportState,
    },
    selection::{AutoScrollDirection, MousePosition, SelectionClickState, SelectionState},
};

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
    pub(crate) tail_layout_cache: TailLayoutCache,
    pub(crate) layout_cache: LayoutCache,
    pub(crate) viewport_cache: ViewportCache,
    pub(crate) follow_bottom: bool,
    pub(crate) manual_scroll: bool,
    pub(crate) restore: RestoreState,
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
