use std::{cell::RefCell, collections::HashMap, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::message_item::UserMessageRenderProjection;
use crate::frontend::tui::styled_text::line_to_plain_text;
use crate::frontend::tui::theme::{TerminalPalette, default_palette};

use super::render_state::{ItemLineAnchor, RenderResult};

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
            line_count: 0,
            plain_line_byte_lens: Rc::new(Vec::new()),
            anchors: CachedLineAnchors::default(),
            plain_text_char_len: 0,
        }
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
    }

    pub(crate) fn extend_lines(&self, target: &mut Vec<Line<'static>>, start: usize, end: usize) {
        if start >= end || start >= self.line_count() {
            return;
        }

        let end = end.min(self.line_count());
        if self.projected_user.is_none() {
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
    }

    pub(crate) fn plain_line_len(&self, index: usize) -> Option<usize> {
        self.plain_line_byte_lens.get(index).copied()
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
                self.lines.get(index).map(|_| ItemLineAnchor {
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

        for block in items.values() {
            summary.estimated_render_ui_bytes += block.estimated_render_ui_bytes();
            summary.estimated_plain_line_bytes +=
                std::mem::size_of_val(block.plain_line_byte_lens.as_slice());
            summary.estimated_anchor_bytes += match &block.anchors {
                CachedLineAnchors::Explicit(anchors) => std::mem::size_of_val(anchors.as_slice()),
                CachedLineAnchors::GeneratedRenderedLines => 0,
            };
        }

        summary
    }

    pub(crate) fn invalidate_result(&mut self) {
        self.valid = false;
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.items.borrow_mut().clear();
        self.dirty_from = 0;
        self.invalidate_result();
    }

    pub(crate) fn reset(&mut self) {
        self.items.borrow_mut().clear();
        self.result = Rc::new(RenderResult::default());
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
        self.items.borrow_mut().remove(&index);
        self.mark_dirty_from(index);
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
}
