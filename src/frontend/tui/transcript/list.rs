use std::{collections::HashMap, rc::Rc, time::Duration};

use ratatui::text::Line;

use super::{
    DEFAULT_RENDER_WIDTH, ItemLineAnchor, RenderResult, TranscriptEstimateBreakdown,
    TranscriptEstimateKind, TranscriptEstimateSource, TranscriptFastEstimate,
    TranscriptItemMetrics, TranscriptItemMetricsCache, TranscriptItemMetricsIndex,
    TranscriptItemMetricsQuality, TranscriptItemPosition,
    cache::{CachedLineAnchors, CachedRenderBlock, MAX_RECENT_RENDER_BLOCKS, ScreenRenderCache},
    new_render_result_with_append_start,
    render_state::RenderItemSummary,
    viewport_overscan_line_budget,
};

#[cfg(test)]
use super::LineAnchorKind;
#[cfg(test)]
use super::ViewportRenderResult;
#[cfg(test)]
use crate::frontend::tui::styled_text::line_to_plain_text;
use crate::frontend::tui::{
    HeroOptions, Sender, StyleMode,
    hero_item::HeroItem,
    message::MessageItem,
    reasoning_message::{ReasoningDisplayMode, ReasoningMessageItem},
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    system_message::SystemMessageItem,
    theme::TerminalPalette,
    tool_result::{ToolResultItem, ToolResultKind},
};

mod block_materialize;
mod metrics_exactize;
mod viewport_materialize;

pub(crate) use block_materialize::materialize_transcript_item_render_block;

/// `TranscriptItem` 表示 transcript 中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptItem {
    Hero(HeroItem),
    Message(MessageItem),
    Reasoning(ReasoningMessageItem),
    System(SystemMessageItem),
    ToolResult(ToolResultItem),
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

    /// `append_assistant_message_with_reasoning` 追加带 reasoning 展示段的助手消息。
    pub(crate) fn append_assistant_message_with_reasoning(
        &mut self,
        content: impl Into<String>,
        reasoning_content: impl Into<String>,
        reasoning_display_mode: ReasoningDisplayMode,
        reasoning_duration: Option<Duration>,
        style_mode: StyleMode,
    ) {
        let content = content.into();
        let reasoning_content = reasoning_content.into();
        if !reasoning_content.is_empty()
            && should_append_reasoning_message(reasoning_display_mode, reasoning_duration)
        {
            self.append_reasoning_message(
                reasoning_content,
                reasoning_display_mode,
                reasoning_duration,
            );
        }
        if !content.is_empty() {
            self.append_message_with_style_mode(Sender::Assistant, content, style_mode);
        }
    }

    /// `append_reasoning_message` 追加一条只展示、不回传给模型的思维链项。
    pub(crate) fn append_reasoning_message(
        &mut self,
        content: impl Into<String>,
        display_mode: ReasoningDisplayMode,
        duration: Option<Duration>,
    ) {
        if !should_append_reasoning_message(display_mode, duration) {
            return;
        }

        self.push_item(TranscriptItem::Reasoning(ReasoningMessageItem::new(
            content,
            display_mode,
            duration,
        )));
    }

    pub(crate) fn is_reasoning_header_hit(
        &self,
        item_index: usize,
        rendered_line: usize,
        column: usize,
    ) -> bool {
        self.items
            .get(item_index)
            .is_some_and(|item| match item.as_ref() {
                TranscriptItem::Reasoning(reasoning) => {
                    reasoning.is_header_line(rendered_line)
                        && column < reasoning.header_display_width()
                }
                TranscriptItem::Hero(_)
                | TranscriptItem::Message(_)
                | TranscriptItem::ToolResult(_)
                | TranscriptItem::System(_) => false,
            })
    }

    pub(crate) fn toggle_reasoning_item(&mut self, item_index: usize) -> bool {
        let Some(item) = self.items.get(item_index) else {
            return false;
        };
        let TranscriptItem::Reasoning(reasoning) = item.as_ref() else {
            return false;
        };
        if !reasoning.is_header_line(0) {
            return false;
        }

        let mut reasoning = reasoning.clone();
        reasoning.toggle();
        Rc::make_mut(&mut self.items)[item_index] = Rc::new(TranscriptItem::Reasoning(reasoning));
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(item_index);
        self.screen_cache.mark_dirty_from(item_index);
        true
    }

    /// `append_system_message` 追加一条只用于 TUI 展示的 system message。
    pub(crate) fn append_system_message(&mut self, content: impl Into<String>) {
        self.push_item(TranscriptItem::System(SystemMessageItem::new(content)));
    }

    /// `append_tool_result` 追加一条只用于 TUI 展示的工具审批结果。
    pub(crate) fn append_tool_result(&mut self, content: impl Into<String>, kind: ToolResultKind) {
        self.push_item(TranscriptItem::ToolResult(ToolResultItem::new(
            content, kind,
        )));
    }

    /// `len` 返回 transcript 项数量。
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// `clear` 清空 transcript。
    pub(crate) fn clear(&mut self) {
        Rc::make_mut(&mut self.items).clear();
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.reset();
        self.screen_cache.reset();
    }

    /// `item` 返回指定索引的 transcript 项。
    #[cfg(test)]
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

    /// `plain_items` 返回适用于纯文本消费的文本项。
    pub(crate) fn plain_items(&self) -> Vec<String> {
        let width = self.render_width();

        self.items
            .iter()
            .map(|item| item.render_plain_text(width, self.palette))
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// `source_messages` 返回 transcript 中可发送给模型的原始对话消息。
    pub(crate) fn source_messages(&self) -> Vec<(Sender, String)> {
        self.items
            .iter()
            .filter_map(|item| match item.as_ref() {
                TranscriptItem::Message(message) => {
                    Some((message.sender(), message.source_content().to_string()))
                }
                TranscriptItem::Hero(_)
                | TranscriptItem::Reasoning(_)
                | TranscriptItem::System(_)
                | TranscriptItem::ToolResult(_) => None,
            })
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

    fn render_width(&self) -> u16 {
        self.width.max(1)
    }

    #[cfg(test)]
    fn can_reuse_cached_render_result(&self, width: u16) -> bool {
        self.screen_cache
            .can_reuse_result(width, self.gap, self.items.len(), self.items_version)
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

fn should_append_reasoning_message(
    display_mode: ReasoningDisplayMode,
    duration: Option<Duration>,
) -> bool {
    !matches!(display_mode, ReasoningDisplayMode::Snippet) || duration.is_some()
}

#[cfg(test)]
mod tests;
