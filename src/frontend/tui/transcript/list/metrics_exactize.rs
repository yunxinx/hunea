use std::rc::Rc;

use super::*;

impl Transcript {
    /// `progressive_item_metrics_index` 返回 transcript 当前宽度下的混合质量索引。
    pub(crate) fn progressive_item_metrics_index(&mut self) -> TranscriptItemMetricsIndex {
        self.progressive_item_metrics_index_impl(false).0
    }

    pub(crate) fn progressive_item_metrics_index_with_breakdown(
        &mut self,
    ) -> (TranscriptItemMetricsIndex, TranscriptEstimateBreakdown) {
        self.progressive_item_metrics_index_impl(true)
    }

    fn progressive_item_metrics_index_impl(
        &mut self,
        collect_breakdown: bool,
    ) -> (TranscriptItemMetricsIndex, TranscriptEstimateBreakdown) {
        let width = self.render_width();
        if self
            .metrics_cache
            .can_reuse(width, self.gap, self.items.len())
        {
            return (
                self.metrics_cache.index.clone(),
                TranscriptEstimateBreakdown::default(),
            );
        }

        let item_count = self.items.len();
        let metrics_dirty_from = self.metrics_cache.metrics_dirty_from.min(item_count);
        let mut estimate_breakdown = TranscriptEstimateBreakdown::default();
        let updated_metrics = (metrics_dirty_from..item_count)
            .map(|index| {
                let previous_metrics = self.metrics_cache.index.metrics.get(index).copied();
                let cache_key = self.items[index].render_cache_key();
                let metrics_entry = if previous_metrics.is_some_and(|metrics| {
                    metrics.is_valid && metrics.width == width && metrics.cache_key == cache_key
                }) {
                    previous_metrics.expect("previous metrics should exist when reusing")
                } else {
                    let estimate_started_at = collect_breakdown.then(std::time::Instant::now);
                    let estimated = self.items[index].estimate_render_metrics_fast(
                        width,
                        self.palette,
                        previous_metrics,
                    );
                    if collect_breakdown {
                        let estimate_time = estimate_started_at
                            .expect("collect_breakdown should capture estimate start time")
                            .elapsed();

                        match self.items[index].as_ref() {
                            TranscriptItem::Hero(_) => {
                                estimate_breakdown.hero_item_count += 1;
                                estimate_breakdown.non_assistant_item_count += 1;
                                estimate_breakdown.hero_estimate_time += estimate_time;
                            }
                            TranscriptItem::System(_) => {
                                estimate_breakdown.non_assistant_item_count += 1;
                            }
                            TranscriptItem::Message(_) => match estimated.kind {
                                TranscriptEstimateKind::Assistant => {
                                    estimate_breakdown.assistant_item_count += 1;
                                    estimate_breakdown.assistant_estimate_time += estimate_time;
                                    if estimated.source == TranscriptEstimateSource::ReusedOnResize
                                    {
                                        estimate_breakdown.assistant_resize_reuse_count += 1;
                                    }
                                }
                                TranscriptEstimateKind::NonAssistant => {
                                    estimate_breakdown.user_item_count += 1;
                                    estimate_breakdown.non_assistant_item_count += 1;
                                    estimate_breakdown.user_estimate_time += estimate_time;
                                    if estimated.source == TranscriptEstimateSource::ReusedOnResize
                                    {
                                        estimate_breakdown.user_resize_reuse_count += 1;
                                    }
                                }
                            },
                        }
                    }
                    TranscriptItemMetrics {
                        item_index: index,
                        width,
                        cache_key,
                        content_line_count: estimated.content_line_count,
                        content_char_len: estimated.content_char_len,
                        quality: TranscriptItemMetricsQuality::Estimated,
                        is_valid: true,
                    }
                };
                (index, metrics_entry)
            })
            .collect::<Vec<_>>();
        {
            let metrics = Rc::make_mut(&mut self.metrics_cache.index.metrics);
            resize_metrics(metrics, item_count);
            for (index, metrics_entry) in updated_metrics {
                metrics[index] = metrics_entry;
            }
        }
        self.rebuild_content_prefix_sums_from(metrics_dirty_from, item_count);
        self.rebuild_visible_positions_from(self.metrics_cache.positions_dirty_from, item_count);
        self.metrics_cache.store_valid(width, self.gap, item_count);

        (self.metrics_cache.index.clone(), estimate_breakdown)
    }

    /// `item_metrics_index` 返回 transcript 当前宽度下的全量精确索引。
    pub(crate) fn item_metrics_index(&mut self) -> TranscriptItemMetricsIndex {
        let _ = self.progressive_item_metrics_index();
        self.exactize_item_range(0, self.items.len());
        self.metrics_cache.index.clone()
    }

    /// `exactize_line_window` 把给定行窗口及 overscan 覆盖到的 item 变成精确 metrics。
    pub(crate) fn exactize_line_window(
        &mut self,
        start_line: usize,
        line_count: usize,
        overscan_lines: usize,
    ) -> Option<(usize, usize)> {
        let index = self.progressive_item_metrics_index();
        if index.line_window_is_exact(start_line, line_count, overscan_lines) {
            return None;
        }

        let (start_position, end_position) =
            index.summary_positions_for_line_window(start_line, line_count, overscan_lines)?;
        let start_item = index.visible_items.get(start_position)?.item_index;
        let end_item = index.visible_items.get(end_position)?.item_index + 1;
        self.exactize_item_range(start_item, end_item);
        Some((start_item, end_item))
    }

    /// `exactize_item_range` 把指定 item 范围更新为当前宽度下的精确 metrics。
    pub(crate) fn exactize_item_range(&mut self, start: usize, end: usize) {
        let _ = self.progressive_item_metrics_index();
        let item_count = self.items.len();
        let start = start.min(item_count);
        let end = end.min(item_count);
        if start >= end {
            return;
        }

        let width = self.render_width();
        let mut updated_metrics = Vec::with_capacity(end - start);
        for index in start..end {
            let cache_key = self.items[index].render_cache_key();
            let (content_line_count, content_char_len) =
                self.items[index].measure_render_metrics(width, self.palette);
            let next_metrics = TranscriptItemMetrics {
                item_index: index,
                width,
                cache_key,
                content_line_count,
                content_char_len,
                quality: TranscriptItemMetricsQuality::Exact,
                is_valid: true,
            };
            if self.metrics_cache.index.metrics.get(index).copied() == Some(next_metrics) {
                continue;
            }
            updated_metrics.push((index, next_metrics));
        }

        if updated_metrics.is_empty() {
            return;
        }

        {
            let metrics = Rc::make_mut(&mut self.metrics_cache.index.metrics);
            for (index, metrics_entry) in updated_metrics {
                metrics[index] = metrics_entry;
            }
        }
        self.rebuild_content_prefix_sums_from(start, item_count);
        self.rebuild_visible_positions_from(start, item_count);
        self.metrics_cache.store_valid(width, self.gap, item_count);
    }

    fn rebuild_content_prefix_sums_from(&mut self, start: usize, item_count: usize) {
        let start = start.min(item_count);
        let prefix_sums = Rc::make_mut(&mut self.metrics_cache.index.content_prefix_sums);
        resize_content_prefix_sums(prefix_sums, item_count);
        for index in start..item_count {
            prefix_sums[index + 1] = prefix_sums[index]
                .saturating_add(self.metrics_cache.index.metrics[index].content_char_len);
        }
        self.metrics_cache.index.content_char_len = *prefix_sums.last().unwrap_or(&0);
    }

    fn rebuild_visible_positions_from(&mut self, start: usize, item_count: usize) {
        let start = start.min(item_count);
        {
            let visible_positions = Rc::make_mut(&mut self.metrics_cache.index.visible_positions);
            resize_visible_positions(visible_positions, item_count);
            if start < item_count {
                visible_positions[start..].fill(usize::MAX);
            }
        }
        {
            let visible_items = Rc::make_mut(&mut self.metrics_cache.index.visible_items);
            let keep_visible_count = visible_items.partition_point(|item| item.item_index < start);
            visible_items.truncate(keep_visible_count);

            let mut total_lines = visible_items
                .last()
                .map(|item| item.start_line + item.total_line_count)
                .unwrap_or(0);
            let mut previous_visible_item_index = visible_items.last().map(|item| item.item_index);

            let visible_positions = Rc::make_mut(&mut self.metrics_cache.index.visible_positions);
            for (index, visible_position) in visible_positions
                .iter_mut()
                .enumerate()
                .take(item_count)
                .skip(start)
            {
                *visible_position = usize::MAX;
                let metrics = self.metrics_cache.index.metrics[index];
                if metrics.content_line_count == 0 {
                    continue;
                }

                let gap_before = usize::from(previous_visible_item_index.is_some()) * self.gap;
                let position = TranscriptItemPosition {
                    item_index: index,
                    start_line: total_lines,
                    gap_before,
                    content_line_count: metrics.content_line_count,
                    total_line_count: gap_before + metrics.content_line_count,
                    content_char_len: metrics.content_char_len,
                    gap_owner_item_index: previous_visible_item_index,
                };
                *visible_position = visible_items.len();
                total_lines += position.total_line_count;
                previous_visible_item_index = Some(index);
                visible_items.push(position);
            }
        }

        self.metrics_cache.index.line_count = self
            .metrics_cache
            .index
            .visible_items
            .last()
            .map(|item| item.start_line + item.total_line_count)
            .unwrap_or(0);
    }
}

fn resize_metrics(metrics: &mut Vec<TranscriptItemMetrics>, item_count: usize) {
    metrics.resize(item_count, TranscriptItemMetrics::default());
}

fn resize_visible_positions(visible_positions: &mut Vec<usize>, item_count: usize) {
    visible_positions.resize(item_count, usize::MAX);
}

fn resize_content_prefix_sums(prefix_sums: &mut Vec<usize>, item_count: usize) {
    if prefix_sums.is_empty() {
        prefix_sums.push(0);
    }
    prefix_sums.resize(item_count.saturating_add(1), 0);
}
