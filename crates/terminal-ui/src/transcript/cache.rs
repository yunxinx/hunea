use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use ratatui::text::Line;

use crate::message::{AssistantMessageRenderProjection, UserMessageRenderProjection};
use crate::styled_text::line_to_plain_text;
use crate::theme::{TerminalPalette, default_palette};

use super::render_state::{ItemLineAnchor, RenderResult};

pub(crate) const MIN_VIEWPORT_OVERSCAN_LINES: usize = 4;
pub(crate) const MAX_VIEWPORT_OVERSCAN_LINES: usize = 12;
pub(crate) const MAX_RECENT_RENDER_BLOCKS: usize = 48;

/// `viewport_overscan_line_budget` 返回 viewport 邻域预热允许扩展的渲染行数。
pub(crate) fn viewport_overscan_line_budget(visible_line_count: usize) -> usize {
    if visible_line_count == 0 {
        return 0;
    }

    visible_line_count.clamp(MIN_VIEWPORT_OVERSCAN_LINES, MAX_VIEWPORT_OVERSCAN_LINES)
}

/// `RetainedBlockMemorySummary` 描述 transcript warmed item block cache 当前驻留的体积拆分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RetainedBlockMemorySummary {
    pub(crate) estimated_render_ui_bytes: usize,
    pub(crate) estimated_plain_line_bytes: usize,
    pub(crate) estimated_anchor_bytes: usize,
    pub(crate) estimated_cache_slot_bytes: usize,
}

/// `CachedLineAnchors` 描述 block 的锚点存储策略。
#[derive(Debug, Clone)]
pub(crate) enum CachedLineAnchors {
    Explicit(Rc<Vec<ItemLineAnchor>>),
    GeneratedRenderedLines,
}

impl Default for CachedLineAnchors {
    fn default() -> Self {
        Self::Explicit(Rc::new(Vec::new()))
    }
}

/// `CachedRenderBlock` 缓存单个 transcript item 在某个宽度下的屏幕渲染结果。
#[derive(Debug, Clone)]
pub(crate) struct CachedRenderBlock {
    pub(crate) cache_key: u64,
    pub(crate) width: u16,
    pub(crate) palette: TerminalPalette,
    pub(crate) lines: Rc<Vec<Line<'static>>>,
    pub(crate) projected_user: Option<Rc<UserMessageRenderProjection>>,
    pub(crate) projected_assistant: Option<Rc<AssistantMessageRenderProjection>>,
    pub(crate) line_count: usize,
    pub(crate) plain_line_byte_lens: Rc<Vec<usize>>,
    pub(crate) anchors: CachedLineAnchors,
    pub(crate) plain_text_char_len: usize,
}

impl Default for CachedRenderBlock {
    fn default() -> Self {
        Self {
            cache_key: 0,
            width: 0,
            palette: default_palette(),
            lines: Rc::new(Vec::new()),
            projected_user: None,
            projected_assistant: None,
            line_count: 0,
            plain_line_byte_lens: Rc::new(Vec::new()),
            anchors: CachedLineAnchors::default(),
            plain_text_char_len: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RecentItemLinks {
    prev: Option<usize>,
    next: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct RecentItemTracker {
    links: HashMap<usize, RecentItemLinks>,
    head: Option<usize>,
    tail: Option<usize>,
}

impl RecentItemTracker {
    fn clear(&mut self) {
        self.links.clear();
        self.head = None;
        self.tail = None;
    }

    fn len(&self) -> usize {
        self.links.len()
    }

    fn touch(&mut self, item_index: usize) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.links.entry(item_index) {
            entry.insert(RecentItemLinks::default());
            self.append_to_tail(item_index);
            return;
        }

        if self.tail == Some(item_index) {
            return;
        }

        self.detach(item_index);
        self.append_to_tail(item_index);
    }

    fn pop_lru(&mut self) -> Option<usize> {
        let head = self.head?;
        let removed = self.remove(head);
        debug_assert!(
            removed,
            "recent item tracker should remove its current head"
        );
        Some(head)
    }

    fn remove(&mut self, item_index: usize) -> bool {
        if !self.links.contains_key(&item_index) {
            return false;
        }

        self.detach(item_index);
        self.links.remove(&item_index);
        true
    }

    fn detach(&mut self, item_index: usize) {
        let Some(links) = self.links.get(&item_index).copied() else {
            return;
        };

        match links.prev {
            Some(prev) => {
                self.links
                    .get_mut(&prev)
                    .expect("recent item tracker prev link should exist")
                    .next = links.next;
            }
            None => {
                self.head = links.next;
            }
        }

        match links.next {
            Some(next) => {
                self.links
                    .get_mut(&next)
                    .expect("recent item tracker next link should exist")
                    .prev = links.prev;
            }
            None => {
                self.tail = links.prev;
            }
        }

        if let Some(current) = self.links.get_mut(&item_index) {
            current.prev = None;
            current.next = None;
        }
    }

    fn append_to_tail(&mut self, item_index: usize) {
        let previous_tail = self.tail;
        match previous_tail {
            Some(tail) => {
                self.links
                    .get_mut(&tail)
                    .expect("recent item tracker tail should exist")
                    .next = Some(item_index);
            }
            None => {
                self.head = Some(item_index);
            }
        }

        let links = self
            .links
            .get_mut(&item_index)
            .expect("recent item tracker entry should exist before append");
        links.prev = previous_tail;
        links.next = None;
        self.tail = Some(item_index);
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct CachedRenderBlockAccessSummary {
    pub(crate) line_reads: usize,
    pub(crate) plain_line_reads: usize,
    pub(crate) anchor_reads: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RecentItemTrackingWorkSummary {
    pub(crate) linear_scan_steps: usize,
    pub(crate) shifted_entries: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CachedRenderBlockAccessKind {
    Line,
    PlainLine,
    Anchor,
}

#[cfg(test)]
thread_local! {
    static TRACKED_CACHED_RENDER_BLOCK_ACCESSES: RefCell<HashMap<u64, CachedRenderBlockAccessSummary>> =
        RefCell::new(HashMap::new());
}

#[cfg(test)]
pub(crate) fn reset_tracked_cached_render_block_access(cache_key: u64) {
    TRACKED_CACHED_RENDER_BLOCK_ACCESSES.with(|tracked| {
        tracked
            .borrow_mut()
            .insert(cache_key, CachedRenderBlockAccessSummary::default());
    });
}

#[cfg(test)]
pub(crate) fn tracked_cached_render_block_access(cache_key: u64) -> CachedRenderBlockAccessSummary {
    TRACKED_CACHED_RENDER_BLOCK_ACCESSES.with(|tracked| {
        tracked
            .borrow()
            .get(&cache_key)
            .copied()
            .unwrap_or_default()
    })
}

#[cfg(test)]
fn record_tracked_cached_render_block_access(cache_key: u64, kind: CachedRenderBlockAccessKind) {
    TRACKED_CACHED_RENDER_BLOCK_ACCESSES.with(|tracked| {
        let mut tracked = tracked.borrow_mut();
        let Some(summary) = tracked.get_mut(&cache_key) else {
            return;
        };
        match kind {
            CachedRenderBlockAccessKind::Line => {
                summary.line_reads += 1;
            }
            CachedRenderBlockAccessKind::PlainLine => {
                summary.plain_line_reads += 1;
            }
            CachedRenderBlockAccessKind::Anchor => {
                summary.anchor_reads += 1;
            }
        }
    });
}

impl CachedRenderBlock {
    pub(crate) fn line_count(&self) -> usize {
        self.line_count
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<Line<'static>> {
        #[cfg(test)]
        record_tracked_cached_render_block_access(
            self.cache_key,
            CachedRenderBlockAccessKind::Line,
        );

        if let Some(line) = self.lines.get(index) {
            return Some(line.clone());
        }

        self.projected_user
            .as_ref()
            .and_then(|projection| projection.line_at(index))
            .or_else(|| {
                self.projected_assistant
                    .as_ref()
                    .and_then(|projection| projection.line_at(index))
            })
    }

    #[cfg(test)]
    pub(crate) fn extend_lines(&self, target: &mut Vec<Line<'static>>, start: usize, end: usize) {
        if start >= end || start >= self.line_count() {
            return;
        }

        let end = end.min(self.line_count());
        if self.projected_user.is_none() && self.projected_assistant.is_none() {
            target.extend(self.lines[start..end].iter().cloned());
            return;
        }

        for index in start..end {
            if let Some(line) = self.line_at(index) {
                target.push(line);
            }
        }
    }

    pub(crate) fn plain_line_at(&self, index: usize) -> Option<String> {
        #[cfg(test)]
        record_tracked_cached_render_block_access(
            self.cache_key,
            CachedRenderBlockAccessKind::PlainLine,
        );

        if let Some(line) = self.lines.get(index) {
            return Some(line_to_plain_text(line));
        }

        self.projected_user
            .as_ref()
            .and_then(|projection| projection.plain_line_at(index))
            .or_else(|| {
                self.projected_assistant
                    .as_ref()
                    .and_then(|projection| projection.plain_line_at(index))
            })
    }

    pub(crate) fn plain_line_len(&self, index: usize) -> Option<usize> {
        self.plain_line_byte_lens.get(index).copied().or_else(|| {
            self.projected_assistant
                .as_ref()
                .and_then(|projection| projection.plain_line_len(index))
        })
    }

    pub(crate) fn anchor_at(&self, index: usize) -> Option<ItemLineAnchor> {
        #[cfg(test)]
        record_tracked_cached_render_block_access(
            self.cache_key,
            CachedRenderBlockAccessKind::Anchor,
        );

        match &self.anchors {
            CachedLineAnchors::Explicit(anchors) => anchors.get(index).copied(),
            CachedLineAnchors::GeneratedRenderedLines => {
                (index < self.line_count()).then_some(ItemLineAnchor {
                    kind: super::render_state::LineAnchorKind::RenderedLine,
                    rendered_line: index,
                    ..ItemLineAnchor::default()
                })
            }
        }
    }

    pub(crate) fn anchor_index(&self, target: ItemLineAnchor) -> Option<usize> {
        match &self.anchors {
            CachedLineAnchors::Explicit(anchors) => {
                let block_index = target.rendered_line;
                if anchors.get(block_index).copied() == Some(target) {
                    return Some(block_index);
                }

                anchors.iter().position(|candidate| *candidate == target)
            }
            CachedLineAnchors::GeneratedRenderedLines => (target.kind
                == super::render_state::LineAnchorKind::RenderedLine)
                .then_some(target.rendered_line)
                .filter(|&index| index < self.line_count()),
        }
    }

    pub(crate) fn estimated_render_ui_bytes(&self) -> usize {
        if let Some(projection) = &self.projected_user {
            return std::mem::size_of_val(self) + projection.estimated_render_ui_bytes();
        }
        if let Some(projection) = &self.projected_assistant {
            return std::mem::size_of_val(self) + projection.estimated_render_ui_bytes();
        }

        std::mem::size_of_val(self)
            + std::mem::size_of_val(self.lines.as_slice())
            + self
                .lines
                .iter()
                .map(|line| {
                    std::mem::size_of_val(line.spans.as_slice())
                        + line
                            .spans
                            .iter()
                            .map(|span| span.content.len())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }

    #[cfg(test)]
    pub(crate) fn stores_plain_lines(&self) -> bool {
        false
    }

    #[cfg(test)]
    pub(crate) fn uses_generated_rendered_line_anchors(&self) -> bool {
        matches!(self.anchors, CachedLineAnchors::GeneratedRenderedLines)
    }
}

/// `ScreenRenderCache` 管理 transcript 的 item 级缓存与整体结果缓存。
#[derive(Debug, Default)]
pub(crate) struct ScreenRenderCache {
    pub(crate) items: Rc<RefCell<HashMap<usize, Rc<CachedRenderBlock>>>>,
    recent_items: Rc<RefCell<RecentItemTracker>>,
    deferred_recent_limit_depth: usize,
    #[cfg(test)]
    recent_item_tracking_work: Rc<RefCell<RecentItemTrackingWorkSummary>>,
    pub(crate) result: Rc<RenderResult>,
    pub(crate) width: u16,
    pub(crate) gap: usize,
    pub(crate) item_count: usize,
    pub(crate) items_version: usize,
    pub(crate) dirty_from: usize,
    pub(crate) valid: bool,
}

impl Clone for ScreenRenderCache {
    fn clone(&self) -> Self {
        Self {
            // 每个 transcript clone 需要独立的命中表，避免 palette 分叉后复用到
            // 另一份实例重新填回的 block；block 本身仍可通过 Rc 共享只读内容。
            items: Rc::new(RefCell::new(self.items.borrow().clone())),
            recent_items: Rc::new(RefCell::new(self.recent_items.borrow().clone())),
            deferred_recent_limit_depth: self.deferred_recent_limit_depth,
            #[cfg(test)]
            recent_item_tracking_work: Rc::new(RefCell::new(
                *self.recent_item_tracking_work.borrow(),
            )),
            result: Rc::clone(&self.result),
            width: self.width,
            gap: self.gap,
            item_count: self.item_count,
            items_version: self.items_version,
            dirty_from: self.dirty_from,
            valid: self.valid,
        }
    }
}

impl ScreenRenderCache {
    pub(crate) fn retained_block_memory_summary(&self) -> RetainedBlockMemorySummary {
        let items = self.items.borrow();
        let mut summary = RetainedBlockMemorySummary {
            estimated_cache_slot_bytes: items.capacity()
                * std::mem::size_of::<(usize, Rc<CachedRenderBlock>)>(),
            ..RetainedBlockMemorySummary::default()
        };
        let mut counted_blocks =
            HashSet::with_capacity(items.len().saturating_add(self.result.items.len()));

        for block in items.values() {
            accumulate_retained_block_memory(&mut summary, &mut counted_blocks, block);
        }
        for item in self.result.items.iter() {
            accumulate_retained_block_memory(&mut summary, &mut counted_blocks, &item.block);
        }

        summary
    }

    pub(crate) fn invalidate_result(&mut self) {
        self.valid = false;
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.items.borrow_mut().clear();
        self.recent_items.borrow_mut().clear();
        self.deferred_recent_limit_depth = 0;
        self.dirty_from = 0;
        self.invalidate_result();
    }

    pub(crate) fn reset(&mut self) {
        self.items.borrow_mut().clear();
        self.recent_items.borrow_mut().clear();
        self.result = Rc::new(RenderResult::default());
        self.deferred_recent_limit_depth = 0;
        self.valid = false;
        self.width = 0;
        self.gap = 0;
        self.item_count = 0;
        self.items_version = 0;
        self.dirty_from = 0;
    }

    pub(crate) fn mark_dirty_from(&mut self, start: usize) {
        self.dirty_from = self.dirty_from.min(start);
        self.invalidate_result();
    }

    #[cfg(test)]
    pub(crate) fn clear_item(&mut self, index: usize) {
        self.remove_item(index);
        self.mark_dirty_from(index);
    }

    pub(crate) fn reusable_item_block(
        &mut self,
        item_index: usize,
        width: u16,
        cache_key: u64,
    ) -> Option<Rc<CachedRenderBlock>> {
        let cached = self.items.borrow().get(&item_index).cloned();
        if let Some(block) = cached.as_ref()
            && block.width == width
            && block.cache_key == cache_key
        {
            self.touch_item(item_index);
            return Some(Rc::clone(block));
        }

        if cached.is_some() {
            self.remove_item(item_index);
        }
        None
    }

    pub(crate) fn store_item_block(&mut self, item_index: usize, block: Rc<CachedRenderBlock>) {
        self.items.borrow_mut().insert(item_index, block);
        self.touch_item(item_index);
        if self.deferred_recent_limit_depth == 0 {
            self.evict_to_recent_limit(MAX_RECENT_RENDER_BLOCKS);
        }
    }

    pub(crate) fn can_reuse_result(
        &self,
        width: u16,
        gap: usize,
        item_count: usize,
        items_version: usize,
    ) -> bool {
        self.valid
            && self.width == width
            && self.gap == gap
            && self.item_count == item_count
            && self.items_version == items_version
    }

    pub(crate) fn store_result(
        &mut self,
        width: u16,
        gap: usize,
        item_count: usize,
        items_version: usize,
        result: Rc<RenderResult>,
    ) {
        self.result = result;
        self.width = width;
        self.gap = gap;
        self.item_count = item_count;
        self.items_version = items_version;
        self.dirty_from = item_count;
        self.valid = true;
    }

    #[cfg(test)]
    pub(crate) fn reset_recent_item_tracking_work(&self) {
        *self.recent_item_tracking_work.borrow_mut() = RecentItemTrackingWorkSummary::default();
    }

    #[cfg(test)]
    pub(crate) fn recent_item_tracking_work(&self) -> RecentItemTrackingWorkSummary {
        *self.recent_item_tracking_work.borrow()
    }

    fn touch_item(&self, item_index: usize) {
        self.recent_items.borrow_mut().touch(item_index);
    }

    pub(crate) fn begin_recent_limit_batch(&mut self) {
        self.deferred_recent_limit_depth = self.deferred_recent_limit_depth.saturating_add(1);
    }

    pub(crate) fn finish_recent_limit_batch(&mut self, limit: usize) {
        if self.deferred_recent_limit_depth == 0 {
            self.evict_to_recent_limit(limit);
            return;
        }

        self.deferred_recent_limit_depth -= 1;
        if self.deferred_recent_limit_depth == 0 {
            self.evict_to_recent_limit(limit);
        }
    }

    fn evict_to_recent_limit(&mut self, limit: usize) {
        if limit == 0 {
            self.items.borrow_mut().clear();
            self.recent_items.borrow_mut().clear();
            return;
        }

        let mut recent_items = self.recent_items.borrow_mut();
        let mut items = self.items.borrow_mut();
        while recent_items.len() > limit {
            let Some(evicted) = recent_items.pop_lru() else {
                break;
            };
            items.remove(&evicted);
        }
    }

    fn remove_item(&mut self, item_index: usize) {
        self.items.borrow_mut().remove(&item_index);
        self.recent_items.borrow_mut().remove(item_index);
    }
}

fn accumulate_retained_block_memory(
    summary: &mut RetainedBlockMemorySummary,
    counted_blocks: &mut HashSet<*const CachedRenderBlock>,
    block: &Rc<CachedRenderBlock>,
) {
    if !counted_blocks.insert(Rc::as_ptr(block)) {
        return;
    }

    summary.estimated_render_ui_bytes += block.estimated_render_ui_bytes();
    summary.estimated_plain_line_bytes +=
        std::mem::size_of_val(block.plain_line_byte_lens.as_slice());
    summary.estimated_anchor_bytes += match &block.anchors {
        CachedLineAnchors::Explicit(anchors) => std::mem::size_of_val(anchors.as_slice()),
        CachedLineAnchors::GeneratedRenderedLines => 0,
    };
}
