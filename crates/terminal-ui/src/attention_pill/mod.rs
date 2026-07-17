//! 左侧常驻 attention pill：非贴底新消息与被遮挡审批的待办提示。
//!
//! 生命周期与右侧 toast 不同：无超时，只在待办条件解除时消失（回到贴底 /
//! 审批面板可见或被处理），并支持鼠标点击直达对应界面。

mod render;
mod state;

#[cfg(test)]
mod tests;

use crossterm::event::MouseButton;
use ratatui::layout::{Position, Rect};

use state::AttentionPillKind;
pub(crate) use state::AttentionPillState;

use super::{Model, modal_layer::ModalLayer, overlay_input_result::OverlayInputResult};

impl Model {
    /// `MessageFinished` 到达但用户看不到消息（非贴底或被全屏层遮挡）时累计计数。
    pub(crate) fn note_message_finished_for_attention_pill(&mut self) {
        if self.top_modal_layer().is_none() && self.document_pinned_to_bottom() {
            return;
        }
        let count = self.attention_pill.new_message_count.unwrap_or(0);
        self.attention_pill.new_message_count = Some(count.saturating_add(1));
    }

    /// 审批面板打开但不可见（被全屏层遮挡或非贴底）时置位审批 pill。
    pub(crate) fn mark_tool_approval_attention_pending(&mut self) {
        self.attention_pill.approval_pending = true;
    }

    /// 审批 pill 直接清除：审批被处理 / 取消（面板关闭）时调用。
    pub(crate) fn clear_tool_approval_attention(&mut self) {
        self.attention_pill.approval_pending = false;
    }

    /// 回到贴底且无全屏层遮挡时，新消息已可见，清除计数。
    ///
    /// 该判定收敛在贴底状态变化的汇聚点（viewport 位置提交与全屏层关闭后），
    /// 不在 render 中回写状态。
    pub(crate) fn clear_new_message_pill_if_pinned(&mut self) {
        if self.attention_pill.new_message_count.is_none() {
            return;
        }
        if self.document_pinned_to_bottom() && self.top_modal_layer().is_none() {
            self.attention_pill.new_message_count = None;
        }
    }

    /// 审批面板恢复可见（层关闭 / 贴底恢复）或已关闭时清除审批 pill。
    pub(crate) fn sync_tool_approval_attention_visibility(&mut self) {
        if !self.attention_pill.approval_pending {
            return;
        }
        let panel_open_but_invisible =
            self.tool_approval_panel_active() && !self.tool_approval_panel_visible();
        if !panel_open_but_invisible {
            self.attention_pill.approval_pending = false;
        }
    }

    /// 会话切换 / 清空的双重置路径统一清空 pill 状态。
    pub(crate) fn reset_attention_pills(&mut self) {
        self.attention_pill = AttentionPillState::default();
    }

    /// pill 浮在最上层：mouse down 命中时优先于模态层与主界面消费。
    pub(crate) fn handle_attention_pill_mouse_down(
        &mut self,
        button: MouseButton,
        column: u16,
        row: u16,
    ) -> OverlayInputResult {
        if button != MouseButton::Left || !self.has_window {
            return OverlayInputResult::Ignored;
        }

        let area = Rect::new(0, 0, self.width, self.height);
        let Some(kind) = self
            .attention_pill_hit_targets(area)
            .into_iter()
            .find(|(_, rect, _)| rect.contains(Position::new(column, row)))
            .map(|(kind, _, _)| kind)
        else {
            return OverlayInputResult::Ignored;
        };

        match kind {
            AttentionPillKind::ToolApproval => {
                self.close_all_non_approval_fullscreen_modal_layers();
                // 跳到底部恢复贴底使内联面板可见；若发生 !pinned -> pinned 转变，
                // 汇聚点已顺带触发延迟升级。
                self.sync_document_viewport_to_bottom();
                // 原本就贴底（仅被全屏层遮挡）时上面不产生转变，这里显式补一次
                // 延迟升级 sync；面板随即获得输入焦点。
                self.sync_tool_approval_preview_mode();
                self.attention_pill.approval_pending = false;
            }
            AttentionPillKind::NewMessages => {
                self.close_all_non_approval_fullscreen_modal_layers();
                self.sync_tool_approval_preview_mode();
                self.sync_document_viewport_to_bottom();
                // 升级后的审批全屏层可能仍在顶层，汇聚点判定不成立，这里显式清除。
                self.attention_pill.new_message_count = None;
                // 回主界面后审批面板同时变为可见，一并收敛审批 pill。
                self.sync_tool_approval_attention_visibility();
            }
        }
        OverlayInputResult::Handled
    }

    /// 逐层关闭全部非审批全屏层；审批全屏预览属于审批流程本身，保留为焦点。
    fn close_all_non_approval_fullscreen_modal_layers(&mut self) {
        loop {
            match self.top_modal_layer() {
                None | Some(ModalLayer::ToolApprovalFullscreenPreview) => break,
                Some(ModalLayer::TranscriptOverlay) => self.close_transcript_overlay(),
                Some(ModalLayer::PromptOverlay) => self.close_prompt_overlay(),
                Some(ModalLayer::SessionPreview) => self.close_session_preview(),
                Some(ModalLayer::SessionPicker) => self.session_picker = None,
                Some(ModalLayer::CopyPicker) => self.copy_picker = None,
                Some(ModalLayer::EntryTree) => self.entry_tree = None,
                Some(ModalLayer::MessageHistory) => self.message_history_picker = None,
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn attention_pill_new_message_count_for_test(&self) -> Option<usize> {
        self.attention_pill.new_message_count
    }

    #[cfg(test)]
    pub(crate) fn attention_pill_approval_pending_for_test(&self) -> bool {
        self.attention_pill.approval_pending
    }
}
