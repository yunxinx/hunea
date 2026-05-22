use std::{cmp::Ordering, rc::Rc, time::Duration};

use super::render_state::RenderItemLines;

/// `TranscriptEstimateKind` 标记 estimated metrics 属于 assistant 还是其它 item。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TranscriptEstimateKind {
    Assistant,
    #[default]
    NonAssistant,
}

/// `TranscriptEstimateSource` 标记 estimated metrics 是重新估算还是在 resize 时复用缓存语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TranscriptEstimateSource {
    #[default]
    Fresh,
    ReusedOnResize,
}

/// `TranscriptFastEstimate` 收敛 estimated metrics 路径返回的轻量结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptFastEstimate {
    pub(crate) content_line_count: usize,
    pub(crate) content_char_len: usize,
    pub(crate) kind: TranscriptEstimateKind,
    pub(crate) source: TranscriptEstimateSource,
}

/// `TranscriptEstimateBreakdown` 收敛 benchmark 关心的 estimated 路径拆分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptEstimateBreakdown {
    pub(crate) assistant_item_count: usize,
    pub(crate) user_item_count: usize,
    pub(crate) hero_item_count: usize,
    pub(crate) other_non_assistant_item_count: usize,
    pub(crate) non_assistant_item_count: usize,
    pub(crate) assistant_resize_reuse_count: usize,
    pub(crate) user_resize_reuse_count: usize,
    pub(crate) assistant_estimate_time: Duration,
    pub(crate) user_estimate_time: Duration,
    pub(crate) hero_estimate_time: Duration,
    pub(crate) other_non_assistant_estimate_time: Duration,
}

/// `TranscriptItemMetricsQuality` 描述当前 metrics 是估算值还是精确值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TranscriptItemMetricsQuality {
    #[default]
    Exact,
    Estimated,
}

/// `TranscriptItemMetrics` 记录单个 item 在当前宽度下的轻量 metrics。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptItemMetrics {
    pub(crate) item_index: usize,
    pub(crate) width: u16,
    pub(crate) cache_key: u64,
    pub(crate) content_line_count: usize,
    pub(crate) content_char_len: usize,
    pub(crate) quality: TranscriptItemMetricsQuality,
    pub(crate) is_valid: bool,
}

impl TranscriptItemMetrics {
    pub(crate) fn is_exact(&self) -> bool {
        self.is_valid && self.quality == TranscriptItemMetricsQuality::Exact
    }

    #[cfg(test)]
    pub(crate) fn is_estimated(&self) -> bool {
        self.is_valid && self.quality == TranscriptItemMetricsQuality::Estimated
    }
}

/// `TranscriptItemPosition` 描述可见 item 在 transcript 中的大致行区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptItemPosition {
    pub(crate) item_index: usize,
    pub(crate) start_line: usize,
    pub(crate) gap_before: usize,
    pub(crate) content_line_count: usize,
    pub(crate) total_line_count: usize,
    pub(crate) content_char_len: usize,
    pub(crate) gap_owner_item_index: Option<usize>,
}

/// `TranscriptItemMetricsIndex` 提供 item metrics 与 offset/item 映射能力。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct TranscriptItemMetricsIndex {
    pub(crate) width: u16,
    pub(crate) gap: usize,
    pub(crate) metrics: Rc<Vec<TranscriptItemMetrics>>,
    pub(crate) visible_items: Rc<Vec<TranscriptItemPosition>>,
    pub(crate) visible_positions: Rc<Vec<usize>>,
    pub(crate) content_prefix_sums: Rc<Vec<usize>>,
    pub(crate) line_count: usize,
    pub(crate) content_char_len: usize,
}

impl TranscriptItemMetricsIndex {
    pub(crate) fn item_lines(&self, item_index: usize) -> Option<RenderItemLines> {
        let position = self.position_for_item(item_index)?;
        Some(RenderItemLines {
            content_start_line: position.start_line + position.gap_before,
            content_line_count: position.content_line_count,
            total_line_count: position.content_line_count
                + self.trailing_gap_line_count(item_index),
        })
    }

    #[cfg(test)]
    pub(crate) fn item_index_for_line(&self, line_index: usize) -> Option<usize> {
        let position = self.position_for_line(line_index)?;
        let relative = line_index.saturating_sub(position.start_line);
        if relative < position.gap_before {
            return position.gap_owner_item_index.or(Some(position.item_index));
        }

        Some(position.item_index)
    }

    pub(crate) fn position_for_item(&self, item_index: usize) -> Option<TranscriptItemPosition> {
        let position = self.summary_position_for_item(item_index)?;
        self.visible_items.get(position).copied()
    }

    pub(crate) fn summary_position_for_item(&self, item_index: usize) -> Option<usize> {
        let position = *self.visible_positions.get(item_index)?;
        (position != usize::MAX).then_some(position)
    }

    pub(crate) fn position_for_line(&self, line_index: usize) -> Option<TranscriptItemPosition> {
        let position = self.summary_position_for_line(line_index)?;
        self.visible_items.get(position).copied()
    }

    pub(crate) fn trailing_gap_line_count(&self, item_index: usize) -> usize {
        let position = match self.summary_position_for_item(item_index) {
            Some(position) => position,
            None => return 0,
        };
        let Some(item) = self.visible_items.get(position) else {
            return 0;
        };
        self.visible_items
            .get(position + 1)
            .filter(|next| next.gap_owner_item_index == Some(item.item_index))
            .map(|next| next.gap_before)
            .unwrap_or(0)
    }

    fn summary_position_for_line(&self, line_index: usize) -> Option<usize> {
        self.visible_items
            .binary_search_by(|item| {
                if line_index < item.start_line {
                    Ordering::Greater
                } else if line_index >= item.start_line + item.total_line_count {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()
    }

    pub(crate) fn summary_positions_for_line_window(
        &self,
        start_line: usize,
        line_count: usize,
        overscan_lines: usize,
    ) -> Option<(usize, usize)> {
        if line_count == 0 || self.line_count == 0 || start_line >= self.line_count {
            return None;
        }

        let visible_end = start_line.saturating_add(line_count).min(self.line_count);
        let overscanned_start = start_line.saturating_sub(overscan_lines);
        let overscanned_end = visible_end
            .saturating_add(overscan_lines)
            .min(self.line_count);
        let last_line = overscanned_end.saturating_sub(1);

        Some((
            self.summary_position_for_line(overscanned_start)?,
            self.summary_position_for_line(last_line)?,
        ))
    }

    #[cfg(test)]
    pub(crate) fn item_range_for_line_window(
        &self,
        start_line: usize,
        line_count: usize,
        overscan_lines: usize,
    ) -> Option<(usize, usize)> {
        let (start_position, end_position) =
            self.summary_positions_for_line_window(start_line, line_count, overscan_lines)?;
        Some((
            self.visible_items.get(start_position)?.item_index,
            self.visible_items.get(end_position)?.item_index + 1,
        ))
    }

    pub(crate) fn line_window_is_exact(
        &self,
        start_line: usize,
        line_count: usize,
        overscan_lines: usize,
    ) -> bool {
        let Some((start_position, end_position)) =
            self.summary_positions_for_line_window(start_line, line_count, overscan_lines)
        else {
            return true;
        };
        self.visible_items[start_position..=end_position]
            .iter()
            .all(|position| self.metrics[position.item_index].is_exact())
    }
}

/// `TranscriptItemMetricsCache` 管理 item metrics/index 的失效边界。
#[derive(Debug, Clone, Default)]
pub(crate) struct TranscriptItemMetricsCache {
    pub(crate) index: TranscriptItemMetricsIndex,
    pub(crate) metrics_dirty_from: usize,
    pub(crate) positions_dirty_from: usize,
    pub(crate) valid: bool,
}

impl TranscriptItemMetricsCache {
    pub(crate) fn reset(&mut self) {
        self.index = TranscriptItemMetricsIndex::default();
        self.metrics_dirty_from = 0;
        self.positions_dirty_from = 0;
        self.valid = false;
    }

    pub(crate) fn mark_metrics_dirty_from(&mut self, start: usize) {
        self.metrics_dirty_from = self.metrics_dirty_from.min(start);
        self.positions_dirty_from = self.positions_dirty_from.min(start);
        self.valid = false;
    }

    pub(crate) fn mark_positions_dirty_from(&mut self, start: usize) {
        self.positions_dirty_from = self.positions_dirty_from.min(start);
        self.valid = false;
    }

    pub(crate) fn invalidate_width(&mut self) {
        self.metrics_dirty_from = 0;
        self.positions_dirty_from = 0;
        self.valid = false;
    }

    pub(crate) fn can_reuse(&self, width: u16, gap: usize, item_count: usize) -> bool {
        self.valid
            && self.index.width == width
            && self.index.gap == gap
            && self.index.metrics.len() == item_count
            && self.index.visible_positions.len() == item_count
            && self.index.content_prefix_sums.len() == item_count.saturating_add(1)
    }

    pub(crate) fn store_valid(&mut self, width: u16, gap: usize, item_count: usize) {
        self.index.width = width;
        self.index.gap = gap;
        self.metrics_dirty_from = item_count;
        self.positions_dirty_from = item_count;
        self.valid = true;
    }
}
