mod input;
pub(crate) mod render;
mod scroll;

#[cfg(test)]
mod tests;

pub(crate) use render::{
    TranscriptOverlayProgressStyle, TranscriptOverlayRenderOptions, build_percentage_rule,
    render_transcript_overlay_view,
};

use crate::{
    Model,
    runner::TerminalMouseModePreference,
    tool_result::ToolActivityRenderMode,
    transcript::{LineAnchor, ReasoningRenderMode, TranscriptItemMetricsIndex},
};

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

/// 打开 overlay 前捕获主界面的语义位置；后续切 detailed 会改变行数，不能直接复用旧 offset。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptOverlayOpenIntent {
    Bottom,
    Offset(usize),
    Anchor(LineAnchor),
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
    #[cfg(test)]
    pub(crate) fn wants_mouse_capture(&self) -> bool {
        matches!(
            self.mouse_mode_preference(),
            TerminalMouseModePreference::Capture
        )
    }

    pub(crate) fn mouse_mode_preference(&self) -> TerminalMouseModePreference {
        self.modal_mouse_mode_preference()
            .unwrap_or(TerminalMouseModePreference::Capture)
    }

    pub(crate) fn open_transcript_overlay(&mut self) {
        if self.transcript_overlay.is_some() {
            return;
        }

        self.complete_startup_banner_entrance();

        // 关闭其它 immersive panel，遵循互斥策略
        self.close_model_panel();
        self.close_tool_approval_panel();
        self.sync_command_panel_navigation();
        self.sync_file_picker_state();
        self.sync_composer_height();

        let compact_index = self.transcript.progressive_item_metrics_index();
        let startup_banner_lines =
            self.transcript_overlay_startup_banner_lines_for_index(&compact_index);
        let content_height = self.transcript_overlay_content_height();
        let open_intent =
            self.capture_transcript_overlay_open_intent(&compact_index, startup_banner_lines);

        // Ctrl+T overlay 是完整 transcript 视图。先记录主界面意图，再切 detailed，
        // 避免 expanded-simplified / compact tool activity 改变行数后沿用旧 offset。
        self.transcript
            .set_tool_activity_render_mode(ToolActivityRenderMode::Detailed);
        self.transcript
            .set_reasoning_render_mode(ReasoningRenderMode::Detailed);
        let scroll_offset =
            self.transcript_overlay_scroll_offset_for_open_intent(open_intent, content_height);

        self.transcript_overlay = Some(TranscriptOverlayState {
            scroll_offset,
            highlight_item_index: None,
        });
    }

    pub(crate) fn close_transcript_overlay(&mut self) {
        if self.transcript_overlay.is_none() {
            return;
        }

        let had_message_revisit_state =
            self.message_revisit.is_armed || self.message_revisit.is_overlay_active;
        self.transcript_overlay = None;
        self.transcript
            .set_tool_activity_render_mode(ToolActivityRenderMode::Compact);
        self.transcript
            .set_reasoning_render_mode(ReasoningRenderMode::Compact);
        if had_message_revisit_state {
            self.reset_message_revisit_state();
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

    fn capture_transcript_overlay_open_intent(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        startup_banner_lines: usize,
    ) -> TranscriptOverlayOpenIntent {
        let document_offset = self.document_runtime.viewport_y;
        if document_offset >= index.line_count {
            return TranscriptOverlayOpenIntent::Bottom;
        }

        if document_offset < startup_banner_lines {
            return TranscriptOverlayOpenIntent::Offset(0);
        }

        let Some(position) = index.position_for_line(document_offset) else {
            return TranscriptOverlayOpenIntent::Offset(
                document_offset.saturating_sub(startup_banner_lines),
            );
        };
        let relative_line = document_offset.saturating_sub(position.start_line);
        if relative_line < position.gap_before {
            return TranscriptOverlayOpenIntent::Offset(
                document_offset.saturating_sub(startup_banner_lines),
            );
        }

        self.transcript
            .materialize_line_anchor(document_offset)
            .1
            .map(TranscriptOverlayOpenIntent::Anchor)
            .unwrap_or_else(|| {
                TranscriptOverlayOpenIntent::Offset(
                    document_offset.saturating_sub(startup_banner_lines),
                )
            })
    }

    fn transcript_overlay_scroll_offset_for_open_intent(
        &mut self,
        intent: TranscriptOverlayOpenIntent,
        content_height: usize,
    ) -> usize {
        match intent {
            TranscriptOverlayOpenIntent::Bottom => {
                let index = self.transcript.progressive_item_metrics_index();
                self.transcript_overlay_max_offset_for_index(&index, content_height)
            }
            TranscriptOverlayOpenIntent::Offset(offset) => {
                let index = self.transcript.progressive_item_metrics_index();
                let max_offset =
                    self.transcript_overlay_max_offset_for_index(&index, content_height);
                offset.min(max_offset)
            }
            TranscriptOverlayOpenIntent::Anchor(anchor) => {
                let (index, line_index) = self.transcript.line_index_for_anchor(anchor);
                let max_offset =
                    self.transcript_overlay_max_offset_for_index(&index, content_height);
                let startup_banner_lines =
                    self.transcript_overlay_startup_banner_lines_for_index(&index);
                line_index
                    .map(|line_index| {
                        line_index
                            .saturating_sub(startup_banner_lines)
                            .min(max_offset)
                    })
                    .unwrap_or(max_offset)
            }
        }
    }
}
