use std::{cmp::Ordering, rc::Rc};

use super::render_state::RenderItemLines;

/// `TranscriptItemMetrics` 记录单个 item 在当前宽度下的轻量 metrics。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptItemMetrics {
    pub(crate) item_index: usize,
    pub(crate) width: u16,
    pub(crate) cache_key: u64,
    pub(crate) content_line_count: usize,
    pub(crate) content_char_len: usize,
    pub(crate) is_valid: bool,
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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
