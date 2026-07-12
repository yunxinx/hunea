use ratatui::text::Line;

use super::*;

#[derive(Debug, Clone)]
pub(crate) struct TranscriptViewportLine {
    pub(crate) line: Line<'static>,
    pub(crate) is_assistant: bool,
    pub(crate) item_index: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptViewportLines {
    pub(crate) index: TranscriptItemMetricsIndex,
    pub(crate) lines: Vec<TranscriptViewportLine>,
}

impl Transcript {
    /// 物化单条 transcript 行的语义锚点，用于跨展示模式恢复同一 source 位置。
    pub(crate) fn materialize_line_anchor(
        &mut self,
        line_index: usize,
    ) -> (TranscriptItemMetricsIndex, Option<LineAnchor>) {
        let context = crate::frame_time::FrameRenderContext::capture();
        let mut index = self.progressive_item_metrics_index();
        if index.line_count == 0 || line_index >= index.line_count {
            return (index, None);
        }

        index = self.exactize_line_window_until_stable(
            index,
            line_index,
            1,
            viewport_overscan_line_budget(1),
        );
        let Some(position) = index.position_for_line(line_index) else {
            return (index, None);
        };
        let relative = line_index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return (
                index,
                Some(LineAnchor {
                    item_index: position.gap_owner_item_index.unwrap_or(position.item_index),
                    item_anchor: ItemLineAnchor {
                        kind: LineAnchorKind::ItemGap,
                        gap_offset: relative,
                        ..ItemLineAnchor::default()
                    },
                }),
            );
        }

        let block_index = relative.saturating_sub(position.gap_before);
        self.begin_recent_render_block_batch();
        let block = self.render_screen_block(position.item_index, self.render_width(), context);
        let anchor = block.anchor_at(block_index).map(|item_anchor| LineAnchor {
            item_index: position.item_index,
            item_anchor,
        });
        self.finish_recent_render_block_batch(1);
        (index, anchor)
    }

    /// 在当前展示模式下把语义锚点反查回 transcript 行号。
    pub(crate) fn line_index_for_anchor(
        &mut self,
        target: LineAnchor,
    ) -> (TranscriptItemMetricsIndex, Option<usize>) {
        let context = crate::frame_time::FrameRenderContext::capture();
        let mut index = self.progressive_item_metrics_index();
        let Some(item_lines) = index.item_lines(target.item_index) else {
            return (index, None);
        };

        let content_line_count = item_lines.content_line_count.max(1);
        index = self.exactize_line_window_until_stable(
            index,
            item_lines.content_start_line,
            content_line_count,
            viewport_overscan_line_budget(content_line_count),
        );

        let Some(item_lines) = index.item_lines(target.item_index) else {
            return (index, None);
        };
        if matches!(target.item_anchor.kind, LineAnchorKind::ItemGap) {
            let gap_line_count = index.trailing_gap_line_count(target.item_index);
            if target.item_anchor.gap_offset >= gap_line_count {
                return (index, None);
            }
            return (
                index,
                Some(
                    item_lines
                        .content_start_line
                        .saturating_add(item_lines.content_line_count)
                        .saturating_add(target.item_anchor.gap_offset),
                ),
            );
        }

        self.begin_recent_render_block_batch();
        let block = self.render_screen_block(target.item_index, self.render_width(), context);
        let line_index = block
            .anchor_index(target.item_anchor)
            .map(|block_index| item_lines.content_start_line.saturating_add(block_index));
        self.finish_recent_render_block_batch(1);
        (index, line_index)
    }

    /// `materialize_line_window` 只物化指定 transcript 行窗口覆盖到的 item。
    pub(crate) fn materialize_line_window(
        &mut self,
        start: usize,
        count: usize,
        context: crate::frame_time::FrameRenderContext,
    ) -> TranscriptViewportLines {
        let mut index = self.progressive_item_metrics_index();
        if count == 0 || index.line_count == 0 || start >= index.line_count {
            return TranscriptViewportLines {
                index,
                lines: Vec::new(),
            };
        }

        let visible_line_count = count.min(index.line_count - start);
        let overscan_lines = viewport_overscan_line_budget(visible_line_count);
        index = self.exactize_line_window_until_stable(
            index,
            start,
            visible_line_count,
            overscan_lines,
        );

        self.begin_recent_render_block_batch();
        let warmed_item_count =
            self.prewarm_viewport_window(&index, start, visible_line_count, context);
        let lines = self.materialize_viewport_lines(&index, start, visible_line_count, context);
        self.finish_recent_render_block_batch(warmed_item_count);

        TranscriptViewportLines { index, lines }
    }

    fn exactize_line_window_until_stable(
        &mut self,
        mut index: TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        overscan_lines: usize,
    ) -> TranscriptItemMetricsIndex {
        let mut remaining_items = index.metrics.len();
        while remaining_items > 0 {
            if index.line_window_is_exact(start, count, overscan_lines) {
                break;
            }

            let Some((start_item, end_item)) =
                self.exactize_line_window(start, count, overscan_lines)
            else {
                break;
            };
            let next_index = self.progressive_item_metrics_index();
            if next_index == index {
                break;
            }

            index = next_index;
            remaining_items = remaining_items.saturating_sub(end_item.saturating_sub(start_item));
        }

        index
    }

    fn materialize_viewport_lines(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        context: crate::frame_time::FrameRenderContext,
    ) -> Vec<TranscriptViewportLine> {
        if count == 0 || index.line_count == 0 || start >= index.line_count {
            return Vec::new();
        }

        let mut remaining = count.min(index.line_count - start);
        let mut lines = Vec::with_capacity(remaining);
        let mut position_index = match index
            .position_for_line(start)
            .and_then(|position| index.summary_position_for_item(position.item_index))
        {
            Some(position_index) => position_index,
            None => return Vec::new(),
        };
        let width = self.render_width();
        let mut line_offset = start.saturating_sub(index.visible_items[position_index].start_line);

        while remaining > 0 {
            let Some(position) = index.visible_items.get(position_index).copied() else {
                break;
            };
            let taken = remaining.min(position.total_line_count.saturating_sub(line_offset));
            let gap_start = line_offset.min(position.gap_before);
            let gap_end = (line_offset + taken).min(position.gap_before);
            for _ in gap_start..gap_end {
                lines.push(TranscriptViewportLine {
                    line: Line::raw(""),
                    is_assistant: false,
                    item_index: None,
                });
            }

            let block_start = line_offset.saturating_sub(position.gap_before);
            let block_end = (line_offset + taken)
                .saturating_sub(position.gap_before)
                .min(position.content_line_count);
            if block_start < block_end {
                let block = self.render_screen_block(position.item_index, width, context);
                let is_assistant = self
                    .items
                    .get(position.item_index)
                    .map(|item| item.is_assistant_message())
                    .unwrap_or(false);
                for block_index in block_start..block_end {
                    if let Some(line) = block.line_at(block_index) {
                        lines.push(TranscriptViewportLine {
                            line,
                            is_assistant,
                            item_index: Some(position.item_index),
                        });
                    }
                }
            }

            remaining -= taken;
            position_index += 1;
            line_offset = 0;
        }

        lines
    }

    /// `render_viewport` 返回 transcript 的可视切片。
    #[cfg(test)]
    pub(crate) fn render_viewport(
        &mut self,
        offset: usize,
        height: usize,
        context: crate::frame_time::FrameRenderContext,
    ) -> ViewportRenderResult {
        self.begin_recent_render_block_batch();
        let index = self.item_metrics_index();
        if index.line_count == 0 {
            self.finish_recent_render_block_batch(0);
            return ViewportRenderResult::default();
        }

        let resolved_offset = if height == 0 || height >= index.line_count {
            0
        } else {
            offset.min(index.line_count.saturating_sub(height))
        };
        let visible_line_count = if height == 0 || height >= index.line_count {
            index.line_count
        } else {
            height
        };
        let warmed_item_count =
            self.prewarm_viewport_window(&index, resolved_offset, visible_line_count, context);
        let slice =
            self.materialize_viewport_slice(&index, resolved_offset, visible_line_count, context);
        self.finish_recent_render_block_batch(warmed_item_count);

        ViewportRenderResult {
            plain_lines: slice.plain_lines,
        }
    }

    /// `prewarm_viewport_window` 预热当前 viewport 及 overscan 邻域对应的 item block，
    /// 并返回本次触达的 item 数量。
    pub(crate) fn prewarm_viewport_window(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        context: crate::frame_time::FrameRenderContext,
    ) -> usize {
        self.prewarm_viewport_neighborhood(index, start, count, self.render_width(), context)
    }

    fn prewarm_viewport_neighborhood(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        width: u16,
        context: crate::frame_time::FrameRenderContext,
    ) -> usize {
        let overscan_lines = viewport_overscan_line_budget(count);
        let Some((start_position, end_position)) =
            index.summary_positions_for_line_window(start, count, overscan_lines)
        else {
            return 0;
        };

        let item_indices = index.visible_items[start_position..=end_position]
            .iter()
            .map(|position| position.item_index)
            .collect::<Vec<_>>();
        let warmed_item_count = item_indices.len();
        for item_index in item_indices {
            let _ = self.render_screen_block(item_index, width, context);
        }
        warmed_item_count
    }

    #[cfg(test)]
    fn materialize_viewport_slice(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        context: crate::frame_time::FrameRenderContext,
    ) -> crate::transcript::render_state::RenderRangeSlice {
        if count == 0 || index.line_count == 0 || start >= index.line_count {
            return crate::transcript::render_state::RenderRangeSlice::default();
        }

        let mut remaining = count.min(index.line_count - start);
        let mut slice = crate::transcript::render_state::RenderRangeSlice {
            lines: Vec::with_capacity(remaining),
            #[cfg(test)]
            plain_lines: Vec::with_capacity(remaining),
        };
        let mut position_index = match index
            .position_for_line(start)
            .and_then(|position| index.summary_position_for_item(position.item_index))
        {
            Some(position_index) => position_index,
            None => {
                return crate::transcript::render_state::RenderRangeSlice::default();
            }
        };
        let width = self.render_width();
        let mut line_offset = start.saturating_sub(index.visible_items[position_index].start_line);

        while remaining > 0 {
            let Some(position) = index.visible_items.get(position_index).copied() else {
                break;
            };
            let taken = remaining.min(position.total_line_count.saturating_sub(line_offset));
            let gap_start = line_offset.min(position.gap_before);
            let gap_end = (line_offset + taken).min(position.gap_before);
            for _ in gap_start..gap_end {
                slice.lines.push(Line::raw(""));
                #[cfg(test)]
                slice.plain_lines.push(String::new());
            }

            let block_start = line_offset.saturating_sub(position.gap_before);
            let block_end = (line_offset + taken)
                .saturating_sub(position.gap_before)
                .min(position.content_line_count);
            if block_start < block_end {
                let block = self.render_screen_block(position.item_index, width, context);
                block.extend_lines(&mut slice.lines, block_start, block_end);
                #[cfg(test)]
                for block_index in block_start..block_end {
                    if let Some(plain_line) = block.plain_line_at(block_index) {
                        slice.plain_lines.push(plain_line);
                    }
                }
            }

            remaining -= taken;
            position_index += 1;
            line_offset = 0;
        }

        slice
    }
}
