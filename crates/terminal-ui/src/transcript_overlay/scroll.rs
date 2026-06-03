use crate::{
    Model,
    transcript::{TranscriptItem, TranscriptItemMetricsIndex},
};

use super::TranscriptOverlayScrollAnchor;

impl Model {
    /// 计算 transcript 中启动欢迎块占用的行数；该块不进入覆盖层。
    pub(super) fn transcript_overlay_startup_banner_lines_for_index(
        &self,
        index: &TranscriptItemMetricsIndex,
    ) -> usize {
        let Some(first_pos) = index.visible_items.first() else {
            return 0;
        };
        let items = self.transcript.items_snapshot();
        let Some(first_item) = items.get(first_pos.item_index) else {
            return 0;
        };
        if matches!(first_item.as_ref(), TranscriptItem::StartupBanner(_)) {
            first_pos.total_line_count
        } else {
            0
        }
    }

    pub(super) fn transcript_overlay_content_height(&self) -> usize {
        self.height.saturating_sub(2).max(1) as usize
    }

    pub(super) fn transcript_overlay_max_offset_for_index(
        &self,
        index: &TranscriptItemMetricsIndex,
        content_height: usize,
    ) -> usize {
        let startup_banner_lines = self.transcript_overlay_startup_banner_lines_for_index(index);
        let effective_total = index.line_count.saturating_sub(startup_banner_lines);
        effective_total.saturating_sub(content_height)
    }

    pub(crate) fn sync_transcript_overlay_after_transcript_refresh(
        &mut self,
        previous_index: &TranscriptItemMetricsIndex,
        next_index: &TranscriptItemMetricsIndex,
    ) {
        let Some(current_offset) = self
            .transcript_overlay
            .as_ref()
            .map(|overlay| overlay.scroll_offset)
        else {
            return;
        };

        let content_height = self.transcript_overlay_content_height();
        let previous_max_offset =
            self.transcript_overlay_max_offset_for_index(previous_index, content_height);
        let next_max_offset =
            self.transcript_overlay_max_offset_for_index(next_index, content_height);
        let next_offset = if current_offset >= previous_max_offset {
            next_max_offset
        } else {
            current_offset.min(next_max_offset)
        };

        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.scroll_offset = next_offset;
            if overlay
                .highlight_item_index
                .is_some_and(|item_index| item_index >= next_index.metrics.len())
            {
                overlay.highlight_item_index = None;
            }
        }
    }

    pub(crate) fn capture_transcript_overlay_scroll_anchor(
        &mut self,
    ) -> Option<TranscriptOverlayScrollAnchor> {
        let scroll_offset = self.transcript_overlay.as_ref()?.scroll_offset;
        let content_height = self.transcript_overlay_content_height();
        let index = self.transcript.progressive_item_metrics_index();
        let max_offset = self.transcript_overlay_max_offset_for_index(&index, content_height);
        Some(TranscriptOverlayScrollAnchor {
            scroll_offset,
            was_at_bottom: scroll_offset >= max_offset,
        })
    }

    pub(crate) fn restore_transcript_overlay_scroll_anchor(
        &mut self,
        anchor: Option<TranscriptOverlayScrollAnchor>,
    ) {
        let Some(anchor) = anchor else {
            return;
        };
        if self.transcript_overlay.is_none() {
            return;
        }

        let content_height = self.transcript_overlay_content_height();
        let index = self.transcript.progressive_item_metrics_index();
        let max_offset = self.transcript_overlay_max_offset_for_index(&index, content_height);
        let next_offset = if anchor.was_at_bottom {
            max_offset
        } else {
            anchor.scroll_offset.min(max_offset)
        };
        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.scroll_offset = next_offset;
        }
    }

    pub(crate) fn scroll_transcript_overlay_item_into_view(&mut self, item_index: usize) {
        if self.transcript_overlay.is_none() {
            return;
        }

        let content_height = self.transcript_overlay_content_height();
        let index = self.transcript.progressive_item_metrics_index();
        let max_offset = self.transcript_overlay_max_offset_for_index(&index, content_height);
        let startup_banner_lines = self.transcript_overlay_startup_banner_lines_for_index(&index);
        let Some(position) = index.position_for_item(item_index) else {
            return;
        };

        let item_start = position
            .start_line
            .saturating_add(position.gap_before)
            .saturating_sub(startup_banner_lines);
        let item_end = item_start.saturating_add(position.content_line_count.max(1));
        let current_offset = self
            .transcript_overlay
            .as_ref()
            .map(|overlay| overlay.scroll_offset.min(max_offset))
            .unwrap_or_default();
        let viewport_end = current_offset.saturating_add(content_height);

        let next_offset = if item_start < current_offset {
            item_start
        } else if item_end > viewport_end {
            item_end.saturating_sub(content_height)
        } else {
            current_offset
        }
        .min(max_offset);

        if let Some(overlay) = self.transcript_overlay.as_mut() {
            overlay.scroll_offset = next_offset;
        }
    }
}
