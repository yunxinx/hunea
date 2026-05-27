use std::{rc::Rc, time::Instant};

use super::{Model, metrics::TranscriptSyncProfile};
use crate::{
    document::offset_viewport_line_indices,
    status_line::{StatusLineRenderResult, status_line_gap_before, status_line_pair_height},
    style_mode::StyleMode,
    transcript::{TranscriptEstimateBreakdown, index_only_render_result},
};

impl Model {
    pub(crate) fn sync_composer_height(&mut self) {
        let full_height = self.composer.full_height().max(1);
        let mut viewport_height = if !self.has_window || self.height == 0 {
            full_height
        } else {
            full_height.min(self.height.max(1))
        };

        let status_line = self.current_status_line_render_result();
        let status_line_2 = self.current_status_line_2_render_result();
        let command_panel = self.current_inline_command_panel_render_result();
        let model_panel = self.current_inline_model_panel_render_result();
        let tool_approval_panel = self.current_inline_tool_approval_panel_render_result();
        if status_line.has_content
            || status_line_2.has_content
            || command_panel.has_content
            || model_panel.has_content
            || tool_approval_panel.has_content
        {
            if self.document_runtime.follow_bottom && !self.document_runtime.manual_scroll {
                let panel_rows = command_panel.lines.len()
                    + model_panel.lines.len()
                    + tool_approval_panel.lines.len();
                let visible_height = self.bottom_follow_composer_content_line_count(
                    &status_line,
                    &status_line_2,
                    panel_rows,
                );
                viewport_height =
                    viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
            } else {
                let visible_height = self.visible_composer_content_line_count_in_viewport();
                if visible_height > 0 {
                    viewport_height =
                        viewport_height.min(u16::try_from(visible_height).unwrap_or(u16::MAX));
                }
            }
        }

        self.composer.set_height(viewport_height);
    }

    /// `sync_transcript_render` 只刷新 transcript 的 metrics/index 摘要，
    /// 不在 sync 阶段做全文 block materialization。
    pub(crate) fn sync_transcript_render(&mut self) {
        let _ = self.sync_transcript_render_profile_impl(false);
    }

    pub(crate) fn sync_transcript_render_profile(&mut self) -> TranscriptSyncProfile {
        self.sync_transcript_render_profile_impl(true)
    }

    fn sync_transcript_render_profile_impl(
        &mut self,
        collect_breakdown: bool,
    ) -> TranscriptSyncProfile {
        let previous_overlay_index = self
            .transcript_overlay
            .as_ref()
            .map(|_| self.transcript_render.index.clone());

        // metrics-only rebuild 不应保留旧 viewport 预热留下的 render block。
        self.transcript.begin_recent_render_block_batch();
        let estimate_started_at = Instant::now();
        let (index, estimate_breakdown) = if collect_breakdown {
            self.transcript
                .progressive_item_metrics_index_with_breakdown()
        } else {
            (
                self.transcript.progressive_item_metrics_index(),
                TranscriptEstimateBreakdown::default(),
            )
        };
        let estimate_time = estimate_started_at.elapsed();
        self.transcript.finish_recent_render_block_batch(0);
        let visible_exact_started_at = Instant::now();
        let index = self.exactize_visible_transcript_window_until_stable(index);
        let visible_exact_time = visible_exact_started_at.elapsed();
        let next_overlay_index = index.clone();
        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.invalidate_document_viewport_cache();
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        if let Some(previous_overlay_index) = previous_overlay_index.as_ref() {
            self.sync_transcript_overlay_after_transcript_refresh(
                previous_overlay_index,
                &next_overlay_index,
            );
        }
        TranscriptSyncProfile {
            estimate_time,
            visible_exact_time,
            estimate_breakdown,
        }
    }

    pub(crate) fn ensure_current_transcript_window_exact(&mut self) {
        // render 阶段 exactization 发生在 layout 构建内部，不能再递归抓当前 layout；
        // 这里直接复用现有 viewport 状态作为手动滚动恢复锚点。
        let preserved_viewport_state = self
            .document_runtime
            .manual_scroll
            .then(|| self.document_runtime.viewport_state.clone());
        let index = self
            .exactize_visible_transcript_window_until_stable(self.transcript_render.index.clone());
        if index == self.transcript_render.index {
            return;
        }

        self.transcript_render = Rc::new(index_only_render_result(index));
        self.transcript_render_version += 1;
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        self.document_runtime.viewport_cache = Default::default();
        self.sync_document_viewport_after_transcript_refresh(preserved_viewport_state);
    }

    pub(super) fn exactize_visible_transcript_window_until_stable(
        &mut self,
        mut index: crate::transcript::TranscriptItemMetricsIndex,
    ) -> crate::transcript::TranscriptItemMetricsIndex {
        let mut remaining_items = index.metrics.len();
        while remaining_items > 0 {
            let Some((start, count)) = self.current_visible_transcript_window_for_index(&index)
            else {
                break;
            };
            let overscan_lines = crate::transcript::viewport_overscan_line_budget(count);
            if index.line_window_is_exact(start, count, overscan_lines) {
                break;
            }

            drop(index);
            self.release_transcript_index_holders_for_exactization();
            let Some((start_item, end_item)) =
                self.transcript
                    .exactize_line_window(start, count, overscan_lines)
            else {
                index = self.transcript.progressive_item_metrics_index();
                break;
            };
            let next_index = self.transcript.progressive_item_metrics_index();
            index = next_index;
            remaining_items = remaining_items.saturating_sub(end_item.saturating_sub(start_item));
        }

        index
    }

    fn release_transcript_index_holders_for_exactization(&mut self) {
        self.transcript_render = Rc::new(index_only_render_result(
            crate::transcript::TranscriptItemMetricsIndex::default(),
        ));
        self.document_runtime.transcript_cache = Default::default();
        self.document_runtime.layout_cache = Default::default();
        self.document_runtime.viewport_cache = Default::default();
    }

    pub(crate) fn status_line_revision(&self) -> usize {
        self.status_line_revision
    }

    pub(crate) fn bump_status_line_revision(&mut self) {
        self.status_line_revision = self.status_line_revision.saturating_add(1);
    }

    fn bottom_follow_composer_content_line_count(
        &self,
        status_line: &StatusLineRenderResult,
        status_line_2: &StatusLineRenderResult,
        panel_rows: usize,
    ) -> usize {
        let viewport_height = usize::from(self.height.max(1));
        let stream_activity = self.current_stream_activity_render_result();
        let mut tail_rows = panel_rows;
        if stream_activity.has_content {
            tail_rows += 1;
        }
        tail_rows += status_line_pair_height(
            status_line,
            status_line_2,
            status_line_gap_before(self.style_mode),
        );
        if self.composer_uses_rendered_frame_padding() {
            tail_rows += 1;
        }

        if tail_rows < viewport_height {
            viewport_height - tail_rows
        } else {
            viewport_height
        }
    }

    pub(crate) fn composer_uses_rendered_frame_padding(&self) -> bool {
        match self.style_mode {
            StyleMode::Cx => self.palette.surface.is_some(),
            StyleMode::Cc => true,
            StyleMode::Ms => false,
        }
    }

    fn visible_composer_content_line_count_in_viewport(&mut self) -> usize {
        let layout = self.build_document_layout();
        let line_indices = offset_viewport_line_indices(
            &layout,
            self.document_runtime.viewport_y,
            self.document_viewport_height(),
        );

        line_indices
            .into_iter()
            .filter(|line_index| {
                *line_index >= layout.composer_slot.content_start_line
                    && *line_index <= layout.composer_slot.content_bottom_line()
            })
            .count()
    }

    /// `current_visible_transcript_window` 返回当前 document viewport 与 transcript 的交集窗口。
    pub(crate) fn current_visible_transcript_window(
        &mut self,
        transcript_line_count: usize,
    ) -> Option<(usize, usize)> {
        if transcript_line_count == 0 || self.document_viewport_height() == 0 {
            return None;
        }

        if self.document_runtime.viewport_state.manual_scroll() {
            let index = self.transcript.progressive_item_metrics_index();
            return self.current_visible_transcript_window_for_index(&index);
        }

        let layout = self.transcript_window_layout(transcript_line_count);
        self.current_visible_transcript_window_for_layout(&layout, transcript_line_count, false)
    }

    pub(super) fn current_visible_transcript_window_for_index(
        &mut self,
        index: &crate::transcript::TranscriptItemMetricsIndex,
    ) -> Option<(usize, usize)> {
        if index.line_count == 0 || self.document_viewport_height() == 0 {
            return None;
        }

        let manual_scroll = self.document_runtime.viewport_state.manual_scroll();
        let layout = self.document_layout_for_transcript_index(index.clone());
        self.current_visible_transcript_window_for_layout(&layout, index.line_count, manual_scroll)
    }

    fn current_visible_transcript_window_for_layout(
        &self,
        layout: &crate::document::DocumentLayout,
        transcript_line_count: usize,
        manual_scroll: bool,
    ) -> Option<(usize, usize)> {
        let document_offset = if manual_scroll {
            self.document_runtime
                .viewport_state
                .resolve_offset_for_current_geometry(
                    layout,
                    self.document_viewport_height(),
                    self.width,
                )
        } else {
            self.document_runtime.viewport_state.resolved_offset()
        };
        let line_indices = self.document_viewport_line_indices_for_mode(
            layout,
            document_offset,
            self.document_runtime.viewport_state.follow_bottom(),
            manual_scroll,
        );

        let mut start = None;
        let mut count = 0usize;
        for line_index in line_indices {
            if line_index >= transcript_line_count {
                if start.is_some() {
                    break;
                }
                continue;
            }

            start.get_or_insert(line_index);
            count += 1;
        }

        start.map(|start| (start, count))
    }
}
