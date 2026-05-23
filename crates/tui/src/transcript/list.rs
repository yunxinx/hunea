use std::{
    collections::{BTreeSet, HashMap},
    rc::Rc,
    time::{Duration, Instant},
};

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
use crate::styled_text::line_to_plain_text;
use crate::{
    HeroOptions, Sender, StyleMode,
    hero_item::HeroItem,
    message::MessageItem,
    reasoning_message::{ReasoningDisplayMode, ReasoningMessageItem},
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    system_message::SystemMessageItem,
    theme::TerminalPalette,
    tool_result::{ToolActivityRenderMode, ToolResultItem, ToolResultKind},
    work_duration_message::WorkDurationMessageItem,
};
use mo_core::session::ChatMessage;
use mo_core::session::{RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityUpdate};

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
    WorkDuration(WorkDurationMessageItem),
}

/// `Transcript` 管理 document-flow 顺序、宽度与逐项渲染缓存。
#[derive(Debug, Clone)]
pub(crate) struct Transcript {
    items: Rc<Vec<Rc<TranscriptItem>>>,
    gap: usize,
    width: u16,
    palette: TerminalPalette,
    tool_activity_render_mode: ToolActivityRenderMode,
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
            && self.tool_activity_render_mode == other.tool_activity_render_mode
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
            tool_activity_render_mode: ToolActivityRenderMode::Compact,
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
        self.append_message_with_style_mode_and_source(sender, content, style_mode, None);
    }

    /// `append_message_with_style_mode_and_source` 追加一条带样式和源消息的消息项。
    pub(crate) fn append_message_with_style_mode_and_source(
        &mut self,
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
        source_message: Option<ChatMessage>,
    ) {
        self.push_item(TranscriptItem::Message(
            MessageItem::new_with_style_mode_and_source(
                sender,
                content,
                style_mode,
                source_message,
            ),
        ));
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
                | TranscriptItem::System(_)
                | TranscriptItem::WorkDuration(_) => false,
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

    /// `append_work_duration_message` 追加一条只用于 TUI 展示的单轮耗时分割线。
    pub(crate) fn append_work_duration_message(&mut self, duration: Duration) {
        self.push_item(TranscriptItem::WorkDuration(WorkDurationMessageItem::new(
            duration,
        )));
    }

    /// `append_tool_result` 追加一条只用于 TUI 展示的工具审批结果。
    pub(crate) fn append_tool_result(&mut self, content: impl Into<String>, kind: ToolResultKind) {
        let mut item = ToolResultItem::new(content, kind);
        item.set_render_mode(self.tool_activity_render_mode);
        self.push_item(TranscriptItem::ToolResult(item));
    }

    /// `append_runtime_tool_activity` 追加一条可更新的 runtime tool activity 展示项。
    pub(crate) fn append_runtime_tool_activity(
        &mut self,
        call: impl Into<RuntimeToolActivity>,
    ) -> usize {
        let call = call.into();
        if let Some(exploration) = ToolResultItem::from_exploration_tool_activity(
            call.clone(),
            self.tool_activity_render_mode,
        ) {
            if let Some((last_index, last_item)) = self.items.iter().enumerate().next_back()
                && let TranscriptItem::ToolResult(tool_result) = last_item.as_ref()
            {
                let mut tool_result = tool_result.clone();
                if tool_result.append_exploration_tool_activity(call) {
                    self.replace_item(last_index, TranscriptItem::ToolResult(tool_result));
                    return last_index;
                }
            }

            let index = self.items.len();
            self.push_item(TranscriptItem::ToolResult(exploration));
            return index;
        }

        let index = self.items.len();
        self.push_item(TranscriptItem::ToolResult(
            ToolResultItem::from_runtime_tool_activity(call, self.tool_activity_render_mode),
        ));
        index
    }

    /// `runtime_tool_activity_index` 返回最近一条匹配 activity id 的 tool activity 展示项。
    pub(crate) fn runtime_tool_activity_index(&self, tool_call_id: &str) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, item)| {
                let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
                    return None;
                };
                tool_result
                    .has_runtime_tool_activity_id(tool_call_id)
                    .then_some(index)
            })
    }

    /// `update_runtime_tool_activity` 用 runtime 增量事件替换已有 tool activity 项。
    pub(crate) fn update_runtime_tool_activity(
        &mut self,
        item_index: usize,
        update: impl Into<RuntimeToolActivityUpdate>,
    ) -> bool {
        let update = update.into();
        let Some(item) = self.items.get(item_index) else {
            return false;
        };
        let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
            return false;
        };

        let mut tool_result = tool_result.clone();
        if !tool_result.update_runtime_tool_activity(update) {
            return false;
        }
        self.replace_item(item_index, TranscriptItem::ToolResult(tool_result));
        true
    }

    /// `set_runtime_terminal_snapshot` 刷新引用指定 terminal 的 tool activity 展示项。
    pub(crate) fn set_runtime_terminal_snapshot(
        &mut self,
        snapshot: impl Into<RuntimeTerminalSnapshot>,
    ) -> bool {
        let snapshot = snapshot.into();
        let mut first_dirty: Option<usize> = None;
        let mut items = self.items.as_ref().clone();
        for (item_index, item) in items.iter_mut().enumerate() {
            let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
                continue;
            };
            let mut tool_result = tool_result.clone();
            if !tool_result.set_runtime_terminal_snapshot(snapshot.clone()) {
                continue;
            }
            *item = Rc::new(TranscriptItem::ToolResult(tool_result));
            first_dirty = Some(first_dirty.map_or(item_index, |dirty| dirty.min(item_index)));
        }

        let Some(first_dirty) = first_dirty else {
            return false;
        };
        self.items = Rc::new(items);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(first_dirty);
        self.screen_cache.mark_dirty_from(first_dirty);
        true
    }

    /// `set_tool_activity_render_mode` 切换工具活动在主 transcript 与 overlay 中的详略。
    pub(crate) fn set_tool_activity_render_mode(&mut self, mode: ToolActivityRenderMode) {
        if self.tool_activity_render_mode == mode {
            return;
        }

        self.tool_activity_render_mode = mode;
        let mut first_dirty: Option<usize> = None;
        let mut items = self.items.as_ref().clone();
        for (index, item) in items.iter_mut().enumerate() {
            let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
                continue;
            };
            let mut tool_result = tool_result.clone();
            if !tool_result.set_render_mode(mode) {
                continue;
            }
            *item = Rc::new(TranscriptItem::ToolResult(tool_result));
            first_dirty = Some(first_dirty.map_or(index, |dirty| dirty.min(index)));
        }
        if let Some(first_dirty) = first_dirty {
            self.items = Rc::new(items);
            self.items_version = self.items_version.saturating_add(1);
            self.metrics_cache.mark_metrics_dirty_from(first_dirty);
            self.screen_cache.mark_dirty_from(first_dirty);
        }
    }

    pub(crate) fn active_tool_activity_started_at(&self) -> Option<Instant> {
        self.items
            .iter()
            .filter_map(|item| item.active_marker_started_at())
            .min()
    }

    pub(crate) fn mark_exploration_tool_activities_complete(&mut self) -> bool {
        let mut first_dirty: Option<usize> = None;
        let mut items = self.items.as_ref().clone();
        for (item_index, item) in items.iter_mut().enumerate() {
            let TranscriptItem::ToolResult(tool_result) = item.as_ref() else {
                continue;
            };
            let mut tool_result = tool_result.clone();
            if !tool_result.mark_exploration_complete() {
                continue;
            }
            *item = Rc::new(TranscriptItem::ToolResult(tool_result));
            first_dirty = Some(first_dirty.map_or(item_index, |dirty| dirty.min(item_index)));
        }

        let Some(first_dirty) = first_dirty else {
            return false;
        };
        self.items = Rc::new(items);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(first_dirty);
        self.screen_cache.mark_dirty_from(first_dirty);
        true
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

    /// `truncate_before_item` 删除指定 item 及其后的所有内容。
    pub(crate) fn truncate_before_item(&mut self, item_index: usize) -> bool {
        if item_index >= self.items.len() {
            return false;
        }

        Rc::make_mut(&mut self.items).truncate(item_index);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.reset();
        self.screen_cache.reset();
        true
    }

    /// `remove_items` 删除指定索引处的 transcript 项并保留其余顺序。
    pub(crate) fn remove_items(&mut self, item_indices: &[usize]) -> bool {
        let item_count = self.items.len();
        let remove_indices = item_indices
            .iter()
            .copied()
            .filter(|index| *index < item_count)
            .collect::<BTreeSet<_>>();
        let Some(first_dirty) = remove_indices.first().copied() else {
            return false;
        };

        let retained_items = self
            .items
            .iter()
            .enumerate()
            .filter(|(index, _)| !remove_indices.contains(index))
            .map(|(_, item)| Rc::clone(item))
            .collect::<Vec<_>>();

        self.items = Rc::new(retained_items);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(first_dirty);
        self.screen_cache.mark_dirty_from(first_dirty);
        true
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
    #[cfg(test)]
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
                | TranscriptItem::ToolResult(_)
                | TranscriptItem::WorkDuration(_) => None,
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

    #[cfg(test)]
    pub(crate) fn cached_render_result_item_count_for_test(&self) -> usize {
        self.screen_cache.result.items.len()
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
        // 一旦开始追加新的 transcript 项，之前的 exploration 组就不应再继续保留“待定”颜色。
        let _ = self.mark_exploration_tool_activities_complete();
        let len_before_append = self.items.len();
        Rc::make_mut(&mut self.items).push(Rc::new(item));
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache
            .mark_metrics_dirty_from(len_before_append);
        self.screen_cache.mark_dirty_from(len_before_append);
    }

    fn replace_item(&mut self, index: usize, item: TranscriptItem) {
        Rc::make_mut(&mut self.items)[index] = Rc::new(item);
        self.items_version = self.items_version.saturating_add(1);
        self.metrics_cache.mark_metrics_dirty_from(index);
        self.screen_cache.mark_dirty_from(index);
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
