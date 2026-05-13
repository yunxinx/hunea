mod input;
mod render;
mod scroll;

#[cfg(test)]
mod tests;

use crate::{Model, tool_result::ToolActivityRenderMode};

/// `TranscriptOverlayState` 保存 transcript 覆盖层的滚动与展示状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptOverlayState {
    pub(crate) scroll_offset: usize,
    pub(crate) highlight_item_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptOverlayScrollAnchor {
    scroll_offset: usize,
    was_at_bottom: bool,
}

impl TranscriptOverlayState {
    pub(crate) fn new() -> Self {
        Self {
            scroll_offset: 0,
            highlight_item_index: None,
        }
    }
}

impl Default for TranscriptOverlayState {
    fn default() -> Self {
        Self::new()
    }
}

impl Model {
    pub(crate) fn transcript_overlay_active(&self) -> bool {
        self.transcript_overlay.is_some()
    }

    /// 覆盖层激活时禁用鼠标捕获，以恢复终端模拟器原生选区能力。
    pub(crate) fn wants_mouse_capture(&self) -> bool {
        !self.transcript_overlay_active()
    }

    pub(crate) fn open_transcript_overlay(&mut self) {
        if self.transcript_overlay.is_some() {
            return;
        }

        // 关闭其它 immersive panel，遵循互斥策略
        self.close_model_panel();
        self.close_tool_approval_panel();
        self.acp_panel.is_open = false;
        self.acp_debug_panel.is_open = false;
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_composer_height();

        // 根据主界面当前 viewport 位置计算初始滚动偏移，保持浏览连续性
        let document_offset = self.document_runtime.viewport_y;
        let metrics_index = self.transcript.progressive_item_metrics_index();
        let transcript_line_count = metrics_index.line_count;
        let hero_lines = self.transcript_overlay_hero_lines_for_index(&metrics_index);
        let content_height = self.transcript_overlay_content_height();
        let max_offset =
            self.transcript_overlay_max_offset_for_index(&metrics_index, content_height);

        let scroll_offset = if document_offset >= transcript_line_count {
            // viewport 顶部已滚入 tail 区域（composer/status line），映射到 transcript 底部
            max_offset
        } else {
            // viewport 顶部在 transcript 区域内，保持对应位置；始终跳过 Hero
            document_offset.saturating_sub(hero_lines).min(max_offset)
        };

        self.transcript_overlay = Some(TranscriptOverlayState {
            scroll_offset,
            highlight_item_index: None,
        });
        self.transcript
            .set_tool_activity_render_mode(ToolActivityRenderMode::Detailed);
    }

    pub(crate) fn close_transcript_overlay(&mut self) {
        if self.transcript_overlay.is_none() {
            return;
        }

        let had_backtrack_state = self.backtrack.primed || self.backtrack.overlay_preview_active;
        self.transcript_overlay = None;
        self.transcript
            .set_tool_activity_render_mode(ToolActivityRenderMode::Compact);
        if had_backtrack_state {
            self.reset_backtrack_state();
        }
        self.sync_composer_height();
    }

    pub(crate) fn toggle_transcript_overlay(&mut self) {
        if self.transcript_overlay.is_some() {
            self.close_transcript_overlay();
        } else {
            self.open_transcript_overlay();
        }
    }
}
