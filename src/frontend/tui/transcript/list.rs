use std::{collections::HashMap, rc::Rc};

use ratatui::text::Line;

use super::{
    DEFAULT_RENDER_WIDTH, ItemLineAnchor, RenderResult, TranscriptEstimateBreakdown,
    TranscriptEstimateKind, TranscriptEstimateSource, TranscriptFastEstimate,
    TranscriptItemMetrics, TranscriptItemMetricsCache, TranscriptItemMetricsIndex,
    TranscriptItemMetricsQuality, TranscriptItemPosition, ViewportRenderResult,
    cache::{CachedLineAnchors, CachedRenderBlock, MAX_RECENT_RENDER_BLOCKS, ScreenRenderCache},
    new_render_result_with_append_start,
    render_state::RenderItemSummary,
    viewport_overscan_line_budget,
};

#[cfg(test)]
use super::LineAnchorKind;
use crate::frontend::tui::{
    HeroOptions, Sender, StyleMode,
    hero_item::HeroItem,
    message_item::MessageItem,
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    styled_text::{line_plain_text_len, line_to_plain_text},
    theme::TerminalPalette,
};

/// `TranscriptItem` 表示 transcript 中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptItem {
    Hero(HeroItem),
    Message(MessageItem),
}

/// `Transcript` 管理 document-flow 顺序、宽度与逐项渲染缓存。
#[derive(Debug, Clone)]
pub(crate) struct Transcript {
    items: Rc<Vec<Rc<TranscriptItem>>>,
    gap: usize,
    width: u16,
    palette: TerminalPalette,
    items_version: usize,
    metrics_cache: TranscriptItemMetricsCache,
    screen_cache: ScreenRenderCache,
}

impl PartialEq for Transcript {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
            && self.gap == other.gap
            && self.width == other.width
            && self.palette == other.palette
    }
}

impl Eq for Transcript {}

impl Transcript {
    /// `new` 创建一个空 transcript。
    pub(crate) fn new(palette: TerminalPalette) -> Self {
        Self {
            items: Rc::new(Vec::new()),
            gap: 1,
            width: DEFAULT_RENDER_WIDTH as u16,
            palette,
            items_version: 1,
            metrics_cache: TranscriptItemMetricsCache::default(),
            screen_cache: ScreenRenderCache::default(),
        }
    }

    /// `set_gap` 设置项与项之间的空行数。
    pub(crate) fn set_gap(&mut self, gap: usize) {
        if self.gap == gap {
            return;
        }

        self.gap = gap;
        self.metrics_cache.mark_positions_dirty_from(0);
        self.screen_cache.mark_dirty_from(0);
    }

    /// `set_width` 设置 transcript 的可用宽度。
    pub(crate) fn set_width(&mut self, width: u16) {
        let width = width.max(1);
        if self.width == width {
            return;
        }

        self.width = width;
        self.metrics_cache.invalidate_width();
        self.screen_cache.invalidate_all();
    }

    /// `set_palette` 刷新 transcript 使用的配色。
    pub(crate) fn set_palette(&mut self, palette: TerminalPalette) {
        if self.palette == palette {
            return;
        }

        self.palette = palette;
        // 用户消息在 surface 开关变化时会增减额外 padding/frame 行，旧 metrics
        // 里的 line_count 和 offset 不能继续复用。
        self.metrics_cache.reset();
        self.screen_cache.invalidate_all();
    }

    /// `append_hero` 追加一条 hero 项。
    pub(crate) fn append_hero(&mut self, options: HeroOptions) {
        self.push_item(TranscriptItem::Hero(HeroItem::new(options)));
    }

    /// `append_message` 追加一条消息项。
    #[cfg(test)]
    pub(crate) fn append_message(&mut self, sender: Sender, content: impl Into<String>) {
        self.push_item(TranscriptItem::Message(MessageItem::new(sender, content)));
    }

    /// `append_message_with_style_mode` 追加一条带样式模式的消息项。
    pub(crate) fn append_message_with_style_mode(
        &mut self,
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
    ) {
        self.push_item(TranscriptItem::Message(MessageItem::new_with_style_mode(
            sender, content, style_mode,
        )));
    }

    /// `len` 返回 transcript 项数量。
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// `clear` 清空 transcript。
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        Rc::make_mut(&mut self.items).clear();
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.reset();
        self.screen_cache.reset();
    }

    /// `item` 返回指定索引的 transcript 项。
    #[allow(dead_code)]
    pub(crate) fn item(&self, index: usize) -> Option<&TranscriptItem> {
        self.items.get(index).map(Rc::as_ref)
    }

    pub(crate) fn items_snapshot(&self) -> Rc<Vec<Rc<TranscriptItem>>> {
        Rc::clone(&self.items)
    }

    /// `cached_screen_blocks_snapshot` 返回当前宽度下已预热的 item block 引用表。
    pub(crate) fn cached_screen_blocks_snapshot(
        &self,
    ) -> Rc<std::cell::RefCell<HashMap<usize, Rc<CachedRenderBlock>>>> {
        Rc::clone(&self.screen_cache.items)
    }

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
                    let estimated = self.items[index].estimate_render_metrics_fast(
                        width,
                        self.palette,
                        previous_metrics,
                    );
                    if collect_breakdown {
                        match estimated.kind {
                            TranscriptEstimateKind::Assistant => {
                                estimate_breakdown.assistant_item_count += 1;
                                if estimated.source == TranscriptEstimateSource::ReusedOnResize {
                                    estimate_breakdown.assistant_resize_reuse_count += 1;
                                }
                            }
                            TranscriptEstimateKind::NonAssistant => {
                                estimate_breakdown.non_assistant_item_count += 1;
                            }
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

    /// `plain_items` 返回适用于纯文本消费的文本项。
    pub(crate) fn plain_items(&self) -> Vec<String> {
        let width = self.render_width();

        self.items
            .iter()
            .map(|item| item.render_plain_text(width, self.palette))
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// `terminal_replay_items` 返回适用于退出 AltScreen 后回放到终端的文本项。
    pub(crate) fn terminal_replay_items(&self, preserve_ansi: bool) -> Vec<String> {
        let width = self.render_width();

        self.items
            .iter()
            .map(|item| item.render_for_terminal_replay(width, self.palette, preserve_ansi))
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// `render` 渲染整个 transcript，并返回带锚点的稳定结果。
    /// 这条路径保留给显式需要完整 transcript block/anchor 的冷路径；steady-state
    /// document 主路径应继续走 `item_metrics_index()` 与局部 viewport materialization。
    pub(crate) fn render(&mut self) -> Rc<RenderResult> {
        let width = self.render_width();
        if self
            .screen_cache
            .can_reuse_result(width, self.gap, self.items.len(), self.items_version)
        {
            return Rc::clone(&self.screen_cache.result);
        }
        if self.items.is_empty() {
            let index = self.item_metrics_index();
            let result = Rc::new(RenderResult::default());
            let result = Rc::new(RenderResult {
                index,
                ..(*result).clone()
            });
            self.screen_cache.store_result(
                width,
                self.gap,
                self.items.len(),
                self.items_version,
                Rc::clone(&result),
            );
            return result;
        }

        let dirty_from = self.screen_cache.dirty_from.min(self.items.len());
        let append_start_line = if self.screen_cache.width == width
            && self.screen_cache.gap == self.gap
            && dirty_from >= self.screen_cache.item_count
            && self.items.len() >= self.screen_cache.item_count
            && self.screen_cache.item_count > 0
        {
            isize::try_from(self.screen_cache.result.line_count).unwrap_or(isize::MAX)
        } else {
            -1
        };
        self.screen_cache.begin_recent_limit_batch();
        let index = self.item_metrics_index();
        let result = Rc::new(self.build_render_result(width, dirty_from, append_start_line, index));
        self.screen_cache.store_result(
            width,
            self.gap,
            self.items.len(),
            self.items_version,
            Rc::clone(&result),
        );
        self.screen_cache
            .finish_recent_limit_batch(MAX_RECENT_RENDER_BLOCKS);
        result
    }

    /// `render_viewport` 返回 transcript 的可视切片。
    #[allow(dead_code)]
    pub(crate) fn render_viewport(&mut self, offset: usize, height: usize) -> ViewportRenderResult {
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
            self.prewarm_viewport_window(&index, resolved_offset, visible_line_count);
        let slice = self.materialize_viewport_slice(&index, resolved_offset, visible_line_count);
        self.finish_recent_render_block_batch(warmed_item_count);

        ViewportRenderResult {
            lines: slice.lines,
            plain_lines: {
                #[cfg(test)]
                {
                    slice.plain_lines
                }
                #[cfg(not(test))]
                {
                    Vec::new()
                }
            },
            line_count: slice.line_count,
            total_line_count: index.line_count,
            resolved_offset,
        }
    }

    /// `retained_block_memory_summary` 返回 warmed item block cache 当前仍被 transcript 保留的体积拆分。
    pub(crate) fn retained_block_memory_summary(&self) -> super::RetainedBlockMemorySummary {
        self.screen_cache.retained_block_memory_summary()
    }

    /// `begin_recent_render_block_batch` 延迟 recent block cache 的裁剪，直到调用方完成预热。
    pub(crate) fn begin_recent_render_block_batch(&mut self) {
        self.screen_cache.begin_recent_limit_batch();
    }

    /// `finish_recent_render_block_batch` 在调用方完成预热后恢复 recent cache，
    /// 但不会把本次 viewport 预热窗口立刻逐出。
    pub(crate) fn finish_recent_render_block_batch(&mut self, warmed_item_count: usize) {
        let retained_limit = if warmed_item_count == 0 {
            0
        } else {
            MAX_RECENT_RENDER_BLOCKS.max(warmed_item_count)
        };
        self.screen_cache.finish_recent_limit_batch(retained_limit);
    }

    /// `prewarm_viewport_window` 预热当前 viewport 及 overscan 邻域对应的 item block，
    /// 并返回本次触达的 item 数量。
    pub(crate) fn prewarm_viewport_window(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
    ) -> usize {
        self.prewarm_viewport_neighborhood(index, start, count, self.render_width())
    }

    fn render_width(&self) -> u16 {
        self.width.max(1)
    }

    #[cfg(test)]
    fn can_reuse_cached_render_result(&self, width: u16) -> bool {
        self.screen_cache
            .can_reuse_result(width, self.gap, self.items.len(), self.items_version)
    }

    fn build_render_result(
        &mut self,
        width: u16,
        dirty_from: usize,
        append_start_line: isize,
        index: TranscriptItemMetricsIndex,
    ) -> RenderResult {
        let previous = Rc::clone(&self.screen_cache.result);
        let mut items = Vec::with_capacity(self.items.len());

        if dirty_from > 0 {
            for summary in previous.items.iter() {
                if summary.item_index >= dirty_from {
                    break;
                }
                items.push(summary.clone());
            }
        }

        let start_position = index
            .visible_items
            .partition_point(|item| item.item_index < dirty_from);
        for position in index.visible_items.iter().skip(start_position) {
            let block = self.render_screen_block(position.item_index, width);
            let summary = RenderItemSummary {
                item_index: position.item_index,
                start_line: position.start_line,
                gap_before: position.gap_before,
                content_line_count: position.content_line_count,
                total_line_count: position.total_line_count,
                gap_owner_item_index: position.gap_owner_item_index,
                block,
            };
            items.push(summary);
        }

        new_render_result_with_append_start(items, index, append_start_line)
    }

    fn render_screen_block(&mut self, index: usize, width: u16) -> Rc<CachedRenderBlock> {
        let cache_key = self.items[index].render_cache_key();
        if let Some(cached) = self
            .screen_cache
            .reusable_item_block(index, width, cache_key)
        {
            return cached;
        }

        let block = Rc::new(materialize_transcript_item_render_block(
            self.items[index].as_ref(),
            width,
            self.palette,
        ));
        self.screen_cache.store_item_block(index, Rc::clone(&block));
        block
    }

    fn prewarm_viewport_neighborhood(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
        width: u16,
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
            let _ = self.render_screen_block(item_index, width);
        }
        warmed_item_count
    }

    fn materialize_viewport_slice(
        &mut self,
        index: &TranscriptItemMetricsIndex,
        start: usize,
        count: usize,
    ) -> super::render_state::RenderRangeSlice {
        if count == 0 || index.line_count == 0 || start >= index.line_count {
            return super::render_state::RenderRangeSlice::default();
        }

        let mut remaining = count.min(index.line_count - start);
        let mut slice = super::render_state::RenderRangeSlice {
            lines: Vec::with_capacity(remaining),
            line_count: remaining,
            plain_char_len: 0,
            #[cfg(test)]
            plain_lines: Vec::with_capacity(remaining),
        };
        let mut position_index = match index
            .position_for_line(start)
            .and_then(|position| index.summary_position_for_item(position.item_index))
        {
            Some(position_index) => position_index,
            None => return super::render_state::RenderRangeSlice::default(),
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
                let block = self.render_screen_block(position.item_index, width);
                block.extend_lines(&mut slice.lines, block_start, block_end);
                slice.plain_char_len += (block_start..block_end)
                    .filter_map(|block_index| block.plain_line_len(block_index))
                    .sum::<usize>();
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

    fn push_item(&mut self, item: TranscriptItem) {
        let len_before_append = self.items.len();
        Rc::make_mut(&mut self.items).push(Rc::new(item));
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache
            .mark_metrics_dirty_from(len_before_append);
        self.screen_cache.mark_dirty_from(len_before_append);
    }

    #[cfg(test)]
    fn replace_item_for_test(&mut self, index: usize, item: TranscriptItem) {
        Rc::make_mut(&mut self.items)[index] = Rc::new(item);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(index);
        self.screen_cache.clear_item(index);
    }

    #[cfg(test)]
    pub(crate) fn dirty_from_for_test(&self) -> usize {
        self.screen_cache.dirty_from
    }

    #[cfg(test)]
    pub(crate) fn item_metrics_dirty_from_for_test(&self) -> usize {
        self.metrics_cache.metrics_dirty_from
    }

    #[cfg(test)]
    pub(crate) fn item_positions_dirty_from_for_test(&self) -> usize {
        self.metrics_cache.positions_dirty_from
    }
}

/// `materialize_transcript_item_render_block` 为单个 transcript item 构造稳定的屏幕块。
pub(crate) fn materialize_transcript_item_render_block(
    item: &TranscriptItem,
    width: u16,
    palette: TerminalPalette,
) -> CachedRenderBlock {
    let cache_key = item.render_cache_key();

    if let TranscriptItem::Message(message) = item
        && let Some(projection) = message.render_projection(width, palette)
    {
        let plain_line_byte_lens = projection.plain_line_lens();
        let plain_text_char_len = plain_line_byte_lens.iter().sum();
        let anchors = projection.line_anchors();
        let line_count = projection.line_count();
        return CachedRenderBlock {
            cache_key,
            width,
            palette,
            lines: Rc::new(Vec::new()),
            projected_user: Some(Rc::new(projection)),
            line_count,
            plain_text_char_len,
            plain_line_byte_lens: Rc::new(plain_line_byte_lens),
            anchors: CachedLineAnchors::Explicit(Rc::new(anchors)),
        };
    }

    let lines = item.render_lines(width, palette);
    let anchors = item.render_line_anchors(width, palette);
    let plain_line_byte_lens = lines.iter().map(line_plain_text_len).collect::<Vec<_>>();
    let plain_text_char_len = plain_line_byte_lens.iter().sum();
    let uses_explicit_anchors = anchors.len() == lines.len();
    CachedRenderBlock {
        cache_key,
        width,
        palette,
        plain_text_char_len,
        line_count: lines.len(),
        lines: Rc::new(lines),
        projected_user: None,
        plain_line_byte_lens: Rc::new(plain_line_byte_lens),
        anchors: if uses_explicit_anchors {
            CachedLineAnchors::Explicit(Rc::new(anchors))
        } else {
            CachedLineAnchors::GeneratedRenderedLines
        },
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

impl TranscriptItem {
    fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        match self {
            Self::Hero(item) => item.estimate_render_metrics_fast(width, palette, previous_metrics),
            Self::Message(item) => {
                item.estimate_render_metrics_fast(width, palette, previous_metrics)
            }
        }
    }

    fn measure_render_metrics(&self, width: u16, palette: TerminalPalette) -> (usize, usize) {
        match self {
            Self::Hero(item) => item.measure_render_metrics(width, palette),
            Self::Message(item) => item.measure_render_metrics(width, palette),
        }
    }

    fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self {
            Self::Hero(item) => item.render_lines(width, palette),
            Self::Message(item) => item.render_lines(width, palette),
        }
    }

    fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        match self {
            Self::Hero(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
            Self::Message(item) => item.render_for_terminal_replay(width, palette, preserve_ansi),
        }
    }

    fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        match self {
            Self::Hero(item) => item.render_plain_text(width, palette),
            Self::Message(item) => item.render_plain_text(width, palette),
        }
    }

    pub(crate) fn render_plain_lines(&self, width: u16, palette: TerminalPalette) -> Vec<String> {
        self.render_lines(width, palette)
            .iter()
            .map(line_to_plain_text)
            .collect()
    }

    fn render_line_anchors(&self, width: u16, palette: TerminalPalette) -> Vec<ItemLineAnchor> {
        match self {
            Self::Hero(item) => item.render_line_anchors(width, palette),
            Self::Message(item) => item.render_line_anchors(width, palette),
        }
    }

    pub(crate) fn render_selectable_line_ranges(
        &self,
        width: u16,
        palette: TerminalPalette,
        plain_lines: &[String],
    ) -> Vec<SelectableLineRange> {
        let ranges = match self {
            Self::Hero(_) => Vec::new(),
            Self::Message(item) => item.render_selectable_line_ranges(width, palette),
        };
        if ranges.len() == plain_lines.len() {
            return ranges;
        }

        plain_lines
            .iter()
            .map(|line| {
                normalize_transcript_selectable_range(line, usize::from(width.max(1)), true)
            })
            .collect()
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        match self {
            Self::Hero(item) => item.render_cache_key(),
            Self::Message(item) => item.render_cache_key(),
        }
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        match self {
            Self::Hero(item) => item.source_text_byte_len(),
            Self::Message(item) => item.source_text_byte_len(),
        }
    }
}

#[cfg(test)]
mod tests {
    const EXPECTED_MAX_RECENT_RENDER_BLOCKS: usize = 48;

    use ratatui::text::Span;

    use super::*;
    use crate::frontend::tui::transcript::{
        render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
    };
    use crate::frontend::tui::{
        HeroOptions, StyleMode,
        message_item::{
            message_item_render_cache_key_call_count,
            reset_message_item_render_cache_key_call_count,
            reset_user_message_projection_plain_line_len_call_count,
            user_message_projection_plain_line_len_call_count,
        },
        theme::{default_palette, terminal_default_palette},
    };

    #[test]
    fn render_returns_content_lines_and_line_count() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "one\ntwo",
            ))),
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "three",
            ))),
        ]);

        let result = transcript.render();
        let rendered = result
            .lines_for_range(0, result.line_count)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(rendered, vec!["one", "two", "", "three"]);
        assert_eq!(result.line_count, 4);
    }

    #[test]
    fn item_metrics_index_maps_offsets_and_item_ranges_without_full_render_result() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "one\ntwo",
            ))),
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "three",
            ))),
        ]);

        let index = transcript.item_metrics_index();

        assert_eq!(index.line_count, 4);
        assert_eq!(
            index.item_lines(0),
            Some(super::super::render_state::RenderItemLines {
                content_start_line: 0,
                content_line_count: 2,
                total_line_count: 3,
            })
        );
        assert_eq!(
            index.item_lines(1),
            Some(super::super::render_state::RenderItemLines {
                content_start_line: 3,
                content_line_count: 1,
                total_line_count: 1,
            })
        );
        assert_eq!(index.item_index_for_line(0), Some(0));
        assert_eq!(index.item_index_for_line(2), Some(0));
        assert_eq!(index.item_index_for_line(3), Some(1));
    }

    #[test]
    fn item_metrics_index_matches_materialized_block_metrics_for_mixed_item_types() {
        let palette = default_palette();
        let mut transcript = Transcript::new(palette);
        transcript.set_gap(1);
        transcript.set_width(18);
        transcript.append_hero(HeroOptions {
            app_name: Some("Lumos".to_string()),
            version: Some("v0.1.0".to_string()),
            work_dir: Some("/tmp/phase-e-metrics".to_string()),
            width: 0,
        });
        transcript.append_message(Sender::Assistant, "## Wrapped heading\n\nassistant body");
        transcript.append_message_with_style_mode(
            Sender::User,
            "user message keeps metrics-only rebuild honest",
            StyleMode::Cx,
        );

        let index = transcript.item_metrics_index();

        for (item_index, item) in transcript.items.iter().enumerate() {
            let block = materialize_transcript_item_render_block(
                item.as_ref(),
                transcript.render_width(),
                palette,
            );
            let metrics = index.metrics[item_index];

            assert_eq!(
                metrics.content_line_count,
                block.line_count(),
                "metrics-only path should preserve line_count for item {item_index}"
            );
            assert_eq!(
                metrics.content_char_len, block.plain_text_char_len,
                "metrics-only path should preserve plain_text_char_len for item {item_index}"
            );
        }
    }

    #[test]
    fn item_metrics_index_tracks_invalidation_boundaries() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "first",
            ))),
            Rc::new(TranscriptItem::Message(MessageItem::new(
                Sender::Assistant,
                "second",
            ))),
        ]);

        let _ = transcript.item_metrics_index();
        assert_eq!(transcript.item_metrics_dirty_from_for_test(), 2);
        assert_eq!(transcript.item_positions_dirty_from_for_test(), 2);

        transcript.append_message(Sender::Assistant, "third");
        assert_eq!(transcript.item_metrics_dirty_from_for_test(), 2);
        assert_eq!(transcript.item_positions_dirty_from_for_test(), 2);

        let _ = transcript.item_metrics_index();
        transcript.replace_item_for_test(1, TranscriptItem::Message(static_message("updated")));
        assert_eq!(transcript.item_metrics_dirty_from_for_test(), 1);
        assert_eq!(transcript.item_positions_dirty_from_for_test(), 1);

        let _ = transcript.item_metrics_index();
        transcript.set_gap(2);
        assert_eq!(transcript.item_metrics_dirty_from_for_test(), 3);
        assert_eq!(transcript.item_positions_dirty_from_for_test(), 0);

        let _ = transcript.item_metrics_index();
        transcript.set_width(48);
        assert_eq!(transcript.item_metrics_dirty_from_for_test(), 0);
        assert_eq!(transcript.item_positions_dirty_from_for_test(), 0);
    }

    #[test]
    fn render_append_path_keeps_gap_anchor_on_previous_item() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "first",
        )))]);
        let _ = transcript.render();

        transcript.append_message(Sender::Assistant, "second");
        let result = transcript.render();
        let line_anchors = result.all_line_anchors();

        assert_eq!(line_anchors.len(), 3);
        assert_eq!(line_anchors[1].item_index, 0);
        assert_eq!(line_anchors[1].item_anchor.kind, LineAnchorKind::ItemGap);
    }

    #[test]
    fn render_append_path_marks_append_start_line() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "first",
        )))]);
        let _ = transcript.render();

        transcript.append_message(Sender::Assistant, "second");
        let result = transcript.render();

        assert_eq!(result.append_start_line, 1);
        assert_eq!(result.all_plain_lines(), vec!["first", "", "second"]);
    }

    #[test]
    fn render_builds_gap_anchor_between_visible_blocks() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![
            Rc::new(TranscriptItem::Message(static_message("one"))),
            Rc::new(TranscriptItem::Message(static_message("two"))),
        ]);

        let result = transcript.render();
        let line_anchors = result.all_line_anchors();

        assert_eq!(line_anchors.len(), 3);
        assert_eq!(line_anchors[1].item_index, 0);
        assert_eq!(line_anchors[1].item_anchor.kind, LineAnchorKind::ItemGap);
        assert_eq!(line_anchors[2].item_index, 1);
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn render_perf_smoke_for_large_cached_transcript() {
        use std::hint::black_box;

        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(72);

        for index in 0..64 {
            Rc::make_mut(&mut transcript.items).push(Rc::new(TranscriptItem::Message(
                static_message(&format!(
                "item {index:02}\nalpha beta gamma alpha beta gamma\ndelta epsilon zeta delta epsilon zeta"
            )),
            )));
        }

        for _ in 0..128 {
            black_box(transcript.render());
        }
    }

    #[test]
    fn cached_render_result_can_be_reused_when_item_cache_keys_are_stable() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "cached",
        )))]);

        let _ = transcript.render();

        assert!(transcript.can_reuse_cached_render_result(transcript.render_width()));
    }

    #[test]
    fn cached_render_result_becomes_stale_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "one",
        )))]);

        let _ = transcript.render();
        transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("two")));

        assert!(!transcript.can_reuse_cached_render_result(transcript.render_width()));
    }

    #[test]
    fn render_cache_hit_reuses_underlying_result_storage() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "cached",
        )))]);

        let first = transcript.render();
        let second = transcript.render();

        assert_eq!(first.items.as_ptr(), second.items.as_ptr());
    }

    #[test]
    fn render_cache_hit_does_not_rehash_message_content() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "cached",
        )))]);
        reset_message_item_render_cache_key_call_count();

        let _ = transcript.render();
        let after_first_render = message_item_render_cache_key_call_count();
        let _ = transcript.render();
        let after_second_render = message_item_render_cache_key_call_count();

        assert_eq!(after_first_render, 0);
        assert_eq!(after_second_render, 0);
    }

    #[test]
    fn append_does_not_preallocate_dense_render_cache_slots() {
        let mut transcript = Transcript::new(default_palette());

        for index in 0..64 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        assert_eq!(
            transcript.screen_cache.items.borrow().len(),
            0,
            "append should not grow dense render cache slots before any render happens"
        );
    }

    #[test]
    fn assistant_render_blocks_use_generated_anchors_without_eager_plain_text_cache() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(12);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "alpha beta gamma delta epsilon",
        )))]);

        let render = transcript.render();
        let block = render
            .items
            .first()
            .expect("assistant item should produce a render block")
            .block
            .as_ref();

        assert!(
            !block.stores_plain_lines(),
            "assistant blocks should not keep a second plain-text copy for every rendered line"
        );
        assert!(
            block.uses_generated_rendered_line_anchors(),
            "assistant blocks should synthesize rendered-line anchors instead of storing a fallback anchor vec"
        );
    }

    #[test]
    fn generated_anchor_blocks_still_round_trip_plain_text_and_anchor_lookup() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(12);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "alpha beta gamma delta epsilon",
        )))]);

        let render = transcript.render();
        let rendered = render
            .line_at(1)
            .expect("wrapped assistant message should expose multiple rendered lines");

        assert!(
            !rendered.plain_line.is_empty(),
            "plain text should still be recoverable when the block only stores structured render data"
        );
        assert_eq!(render.line_index_for_anchor(rendered.anchor), Some(1));
    }

    #[test]
    fn user_render_blocks_project_lines_without_eager_styled_line_storage() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(16);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
            MessageItem::new_with_style_mode(
                Sender::User,
                "user message keeps wrapped projection stable across renders",
                StyleMode::Cx,
            ),
        ))]);

        let render = transcript.render();
        let block = render
            .items
            .first()
            .expect("user item should produce a render block")
            .block
            .as_ref();

        assert!(
            block.lines.is_empty(),
            "user blocks should keep a compact projection and materialize styled lines on demand"
        );
    }

    #[test]
    fn projected_user_render_block_reuses_plain_line_lengths_during_cache_population() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(16);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
            MessageItem::new_with_style_mode(
                Sender::User,
                "user message keeps wrapped projection stable across renders",
                StyleMode::Cx,
            ),
        ))]);
        reset_user_message_projection_plain_line_len_call_count();

        let render = transcript.render();
        let block = render
            .items
            .first()
            .expect("user item should produce a render block")
            .block
            .as_ref();

        assert_eq!(
            user_message_projection_plain_line_len_call_count(),
            block.line_count(),
            "projected user cache population should compute each plain line length only once"
        );
    }

    #[test]
    fn projected_user_blocks_still_round_trip_plain_text_and_anchor_lookup() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(16);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(
            MessageItem::new_with_style_mode(
                Sender::User,
                "user message keeps wrapped projection stable across renders",
                StyleMode::Cx,
            ),
        ))]);

        let expected_plain_lines = transcript.items[0]
            .render_lines(16, default_palette())
            .iter()
            .map(line_to_plain_text)
            .collect::<Vec<_>>();
        let render = transcript.render();
        let actual_plain_lines = (0..render.line_count)
            .map(|index| {
                render
                    .line_at(index)
                    .expect("projected user block should materialize every visible line")
                    .plain_line
            })
            .collect::<Vec<_>>();
        let anchor = render
            .line_at(1)
            .expect("projected user block should expose wrapped content lines")
            .anchor;

        assert_eq!(actual_plain_lines, expected_plain_lines);
        assert_eq!(render.line_index_for_anchor(anchor), Some(1));
    }

    #[test]
    fn precomputed_render_cache_key_changes_with_message_content_and_style() {
        let assistant_one = TranscriptItem::Message(MessageItem::new(Sender::Assistant, "one"));
        let assistant_two = TranscriptItem::Message(MessageItem::new(Sender::Assistant, "two"));
        let user_cx = TranscriptItem::Message(MessageItem::new_with_style_mode(
            Sender::User,
            "same",
            StyleMode::Cx,
        ));
        let user_cc = TranscriptItem::Message(MessageItem::new_with_style_mode(
            Sender::User,
            "same",
            StyleMode::Cc,
        ));

        assert_ne!(
            assistant_one.render_cache_key(),
            assistant_two.render_cache_key()
        );
        assert_ne!(user_cx.render_cache_key(), user_cc.render_cache_key());
    }

    #[test]
    fn render_refreshes_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "one",
        )))]);

        let first = transcript.render();
        assert_eq!(first.all_plain_lines(), vec!["one"]);

        transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("two")));

        let second = transcript.render();
        assert_eq!(second.all_plain_lines(), vec!["two"]);
    }

    #[test]
    fn render_viewport_refreshes_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(static_message(
            "one\ntwo",
        )))]);

        let first = transcript.render_viewport(1, 1);
        assert_eq!(first.plain_lines, vec!["two"]);

        transcript.replace_item_for_test(0, TranscriptItem::Message(static_message("alpha\nbeta")));

        let second = transcript.render_viewport(1, 1);
        assert_eq!(second.plain_lines, vec!["beta"]);
    }

    #[test]
    fn item_metrics_index_keeps_recent_render_block_cache_bounded() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        let _ = transcript.item_metrics_index();
        assert!(
            transcript.screen_cache.items.borrow().is_empty(),
            "Phase E metrics rebuild should stay metrics-only and avoid materializing render blocks"
        );
    }

    #[test]
    fn item_metrics_index_avoids_linear_recent_cache_bookkeeping_for_large_batches() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        transcript.screen_cache.reset_recent_item_tracking_work();
        let _ = transcript.item_metrics_index();
        let work = transcript.screen_cache.recent_item_tracking_work();

        assert_eq!(
            work.linear_scan_steps, 0,
            "recent cache tracking should not linearly scan bookkeeping state during large metrics batches: {work:?}"
        );
        assert_eq!(
            work.shifted_entries, 0,
            "recent cache tracking should not shift bookkeeping entries during large metrics batches: {work:?}"
        );
    }

    #[test]
    fn progressive_metrics_resize_keeps_assistant_markdown_on_fast_estimate_path() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(80);

        for index in 0..4 {
            transcript.append_message(
                Sender::Assistant,
                format!(
                    "## Assistant {index}\n\n- keep estimate cheap\n- keep width changes stable\n\n```rust\nfn item_{index}() {{}}\n```"
                ),
            );
        }

        let _ = transcript.item_metrics_index();
        reset_render_markdown_metrics_call_count();

        transcript.set_width(120);
        let (index, breakdown) = transcript.progressive_item_metrics_index_with_breakdown();

        assert_eq!(
            breakdown.assistant_resize_reuse_count, 4,
            "resize should report semantic reuse for every assistant item whose previous metrics were reused"
        );
        assert_eq!(
            render_markdown_metrics_call_count(),
            0,
            "assistant resize should stay on the fast estimate path instead of reparsing Markdown metrics for every cached item"
        );
        assert!(
            index
                .metrics
                .iter()
                .all(TranscriptItemMetrics::is_estimated),
            "resize reuse should keep assistant metrics estimated until the visible window is exactized"
        );
    }

    #[test]
    fn progressive_metrics_assistant_estimate_skips_exact_markdown_metrics_on_cold_resume() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(80);
        transcript.append_message(
            Sender::Assistant,
            "## Overview\n\n- keep the fast path cheap\n- render exactly later",
        );

        reset_render_markdown_metrics_call_count();
        let _ = transcript.progressive_item_metrics_index();

        assert_eq!(
            render_markdown_metrics_call_count(),
            0,
            "progressive assistant metrics should stay on the fast estimate path instead of paying exact Markdown metrics during cold resume"
        );
    }

    #[test]
    fn progressive_metrics_resize_keeps_assistant_line_count_equal_to_exact_metrics() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(10);
        transcript.append_message(Sender::Assistant, "foo  bar baz");

        let _ = transcript.progressive_item_metrics_index();

        transcript.set_width(5);
        let estimated_line_count = transcript.progressive_item_metrics_index().line_count;
        let exact_line_count = transcript.item_metrics_index().line_count;

        assert_eq!(estimated_line_count, exact_line_count);
        assert_eq!(exact_line_count, 3);
    }

    #[test]
    fn progressive_metrics_keep_plain_text_prefix_sums_equal_to_exact_for_tabs() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(9);
        transcript.append_message(Sender::Assistant, "a\tb");
        transcript.append_message(Sender::Assistant, "tail");

        let estimated_index = transcript.progressive_item_metrics_index();
        let exact_index = transcript.item_metrics_index();

        assert_eq!(
            estimated_index.metrics[0].content_char_len,
            exact_index.metrics[0].content_char_len
        );
        assert_eq!(
            estimated_index.content_prefix_sums,
            exact_index.content_prefix_sums
        );
        assert_eq!(
            estimated_index.content_char_len,
            exact_index.content_char_len
        );
    }

    #[test]
    fn progressive_metrics_resize_defers_tabbed_markdown_prefix_sum_exactization() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(20);
        transcript.append_message(Sender::Assistant, "- item with a tab\tand tail");
        transcript.append_message(Sender::Assistant, "tail");

        let exact_before_resize = transcript.item_metrics_index();
        reset_render_markdown_metrics_call_count();

        transcript.set_width(10);
        let estimated_index = transcript.progressive_item_metrics_index();
        assert_eq!(
            render_markdown_metrics_call_count(),
            0,
            "progressive resize should keep tabbed Markdown on the fast estimate path"
        );
        assert!(estimated_index.metrics[0].is_estimated());

        let exact_index = transcript.item_metrics_index();
        assert_eq!(
            render_markdown_metrics_call_count(),
            2,
            "exactization should pay the Markdown metrics cost only when the exact path is requested, including the remaining assistant items in range"
        );

        assert!(exact_index.metrics[0].is_exact());
        assert!(
            estimated_index.metrics[0].content_char_len
                >= exact_before_resize.metrics[0].content_char_len,
            "resize reuse should preserve the previous assistant plain-text length floor until exactization"
        );
        assert!(
            estimated_index.content_char_len >= exact_before_resize.content_char_len,
            "full-range plain-text totals should not shrink while resize reuse is still estimated"
        );
        assert!(
            estimated_index.metrics[0].content_char_len <= exact_index.metrics[0].content_char_len
        );
    }

    #[test]
    fn progressive_metrics_breakdown_counts_assistant_semantic_resize_reuse() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(80);
        transcript.append_message(Sender::Assistant, "make the handler return early");

        let _ = transcript.progressive_item_metrics_index();

        transcript.set_width(20);
        let (_, breakdown) = transcript.progressive_item_metrics_index_with_breakdown();

        assert_eq!(breakdown.assistant_item_count, 1);
        assert_eq!(breakdown.assistant_resize_reuse_count, 1);
    }

    #[test]
    fn progressive_metrics_resize_keeps_reused_assistant_metrics_estimated() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(80);
        transcript.append_message(
            Sender::Assistant,
            "## Resize\n\n- keep resize cheap\n- exactize only the visible window later",
        );

        let _ = transcript.item_metrics_index();
        reset_render_markdown_metrics_call_count();

        transcript.set_width(24);
        let index = transcript.progressive_item_metrics_index();

        assert!(index.metrics[0].is_estimated());
        assert_eq!(
            render_markdown_metrics_call_count(),
            0,
            "progressive resize should not pay a Markdown metrics pass before the visible window requests exactization"
        );
    }

    #[test]
    fn metrics_rebuild_keeps_screen_block_cache_cold_until_render_materialization() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        let _ = transcript.item_metrics_index();
        assert!(
            transcript.screen_cache.items.borrow().is_empty(),
            "metrics rebuild should not prewarm render blocks before a real materialization path asks for them"
        );

        let render = transcript.render();
        assert!(
            !transcript.screen_cache.items.borrow().is_empty(),
            "full render should still populate render blocks once the materialization path runs"
        );
        assert_eq!(render.items.len(), 96);
    }

    #[test]
    fn retained_block_memory_summary_counts_result_owned_blocks_after_full_render() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(
                Sender::Assistant,
                format!("item {index}\nalpha beta gamma delta epsilon"),
            );
        }

        let render = transcript.render();
        let summary = transcript.retained_block_memory_summary();
        let expected = retained_block_memory_summary_for_render(&render, summary);

        assert!(
            render.items.len() > EXPECTED_MAX_RECENT_RENDER_BLOCKS,
            "test fixture should exceed the bounded recent cache size"
        );
        assert_eq!(
            summary.estimated_render_ui_bytes, expected.estimated_render_ui_bytes,
            "retained memory should count every unique block still owned by the render result"
        );
        assert_eq!(
            summary.estimated_plain_line_bytes, expected.estimated_plain_line_bytes,
            "retained memory should include plain-line metadata for result-owned blocks"
        );
        assert_eq!(
            summary.estimated_anchor_bytes, expected.estimated_anchor_bytes,
            "retained memory should include anchor metadata for result-owned blocks"
        );
    }

    #[test]
    fn render_viewport_prewarms_overscan_neighbors_once_metrics_are_warm() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..10 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        let _ = transcript.item_metrics_index();
        transcript.screen_cache.items.borrow_mut().clear();
        transcript.screen_cache.result = Rc::new(RenderResult::default());
        transcript.screen_cache.valid = false;

        let viewport = transcript.render_viewport(5, 1);

        assert_eq!(viewport.plain_lines, vec!["item 5".to_string()]);
        assert_eq!(
            transcript.screen_cache.items.borrow().len(),
            9,
            "viewport materialization should prewarm a bounded overscan neighborhood"
        );
        for expected in 1..=9 {
            assert!(
                transcript
                    .screen_cache
                    .items
                    .borrow()
                    .contains_key(&expected),
                "overscan neighborhood should keep item {expected} warm"
            );
        }
    }

    #[test]
    fn render_viewport_keeps_large_visible_window_warm() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        let visible_count = EXPECTED_MAX_RECENT_RENDER_BLOCKS + 16;
        let viewport = transcript.render_viewport(0, visible_count);

        assert_eq!(viewport.plain_lines.len(), visible_count);
        for expected in 0..visible_count {
            assert!(
                transcript
                    .screen_cache
                    .items
                    .borrow()
                    .contains_key(&expected),
                "large viewport warm cache should retain visible item {expected}"
            );
        }
    }

    #[test]
    fn finish_recent_render_block_batch_evicts_all_warmed_blocks_when_visible_window_is_empty() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_gap(0);
        transcript.set_width(32);

        for index in 0..96 {
            transcript.append_message(Sender::Assistant, format!("item {index}"));
        }

        let visible_count = EXPECTED_MAX_RECENT_RENDER_BLOCKS + 16;
        let viewport = transcript.render_viewport(0, visible_count);
        assert_eq!(viewport.plain_lines.len(), visible_count);
        assert!(
            !transcript.screen_cache.items.borrow().is_empty(),
            "test fixture should start from an already warmed block cache"
        );

        transcript.begin_recent_render_block_batch();
        transcript.finish_recent_render_block_batch(0);

        assert!(
            transcript.screen_cache.items.borrow().is_empty(),
            "empty visible window should evict every warmed block instead of retaining the default recent limit"
        );
    }

    #[test]
    fn cloned_transcript_does_not_reuse_screen_blocks_from_a_different_palette() {
        let mut original = Transcript::new(default_palette());
        original.set_width(20);
        original.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::User,
            "hello",
        )))]);

        let mut cloned = original.clone();
        cloned.set_palette(terminal_default_palette());

        let original_render = original.render();
        assert_eq!(original_render.line_count, 3);

        let cloned_render = cloned.render();
        assert_eq!(cloned_render.line_count, 1);
        assert_eq!(
            cloned_render.all_plain_lines(),
            vec!["› hello             "]
        );
    }

    #[test]
    fn palette_change_invalidates_item_metrics_when_render_shape_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(20);
        transcript.items = Rc::new(vec![Rc::new(TranscriptItem::Message(MessageItem::new(
            Sender::User,
            "hello",
        )))]);

        let initial_index = transcript.item_metrics_index();
        assert_eq!(initial_index.line_count, 3);
        assert_eq!(
            initial_index
                .item_lines(0)
                .map(|lines| lines.content_line_count),
            Some(3)
        );

        transcript.set_palette(terminal_default_palette());

        let updated_index = transcript.item_metrics_index();
        assert_eq!(updated_index.line_count, 1);
        assert_eq!(
            updated_index
                .item_lines(0)
                .map(|lines| lines.content_line_count),
            Some(1)
        );

        let render = transcript.render();
        assert_eq!(render.line_count, 1);
        assert_eq!(render.all_plain_lines(), vec!["› hello             "]);
    }

    fn static_message(content: &str) -> MessageItem {
        MessageItem::new(Sender::Assistant, content)
    }

    #[allow(dead_code)]
    fn styled_line(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }

    fn retained_block_memory_summary_for_render(
        render: &RenderResult,
        actual: super::super::RetainedBlockMemorySummary,
    ) -> super::super::RetainedBlockMemorySummary {
        let mut summary = super::super::RetainedBlockMemorySummary {
            estimated_cache_slot_bytes: actual.estimated_cache_slot_bytes,
            ..super::super::RetainedBlockMemorySummary::default()
        };
        let mut seen = std::collections::HashSet::new();

        for item in render.items.iter() {
            let block_ptr = Rc::as_ptr(&item.block) as usize;
            if !seen.insert(block_ptr) {
                continue;
            }

            let block = item.block.as_ref();
            summary.estimated_render_ui_bytes += block.estimated_render_ui_bytes();
            summary.estimated_plain_line_bytes +=
                std::mem::size_of_val(block.plain_line_byte_lens.as_slice());
            summary.estimated_anchor_bytes += match &block.anchors {
                super::super::CachedLineAnchors::Explicit(anchors) => {
                    std::mem::size_of_val(anchors.as_slice())
                }
                super::super::CachedLineAnchors::GeneratedRenderedLines => 0,
            };
        }

        summary
    }
}
