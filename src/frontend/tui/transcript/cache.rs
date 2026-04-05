use std::{collections::HashMap, rc::Rc};

use ratatui::text::Line;

use crate::frontend::tui::styled_text::line_to_plain_text;

use super::render_state::{ItemLineAnchor, RenderResult};

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
#[derive(Debug, Clone, Default)]
pub(crate) struct CachedRenderBlock {
    pub(crate) cache_key: u64,
    pub(crate) width: u16,
    pub(crate) lines: Rc<Vec<Line<'static>>>,
    pub(crate) plain_line_byte_lens: Rc<Vec<usize>>,
    pub(crate) anchors: CachedLineAnchors,
    pub(crate) plain_text_char_len: usize,
}

impl CachedRenderBlock {
    pub(crate) fn plain_line_at(&self, index: usize) -> Option<String> {
        self.lines.get(index).map(line_to_plain_text)
    }

    pub(crate) fn plain_line_len(&self, index: usize) -> Option<usize> {
        self.plain_line_byte_lens.get(index).copied()
    }

    pub(crate) fn anchor_at(&self, index: usize) -> Option<ItemLineAnchor> {
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
                .filter(|&index| index < self.lines.len()),
        }
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
#[derive(Debug, Clone, Default)]
pub(crate) struct ScreenRenderCache {
    pub(crate) items: HashMap<usize, Rc<CachedRenderBlock>>,
    pub(crate) result: Rc<RenderResult>,
    pub(crate) width: u16,
    pub(crate) gap: usize,
    pub(crate) item_count: usize,
    pub(crate) items_version: usize,
    pub(crate) dirty_from: usize,
    pub(crate) valid: bool,
}

impl ScreenRenderCache {
    pub(crate) fn invalidate_result(&mut self) {
        self.valid = false;
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.items.clear();
        self.dirty_from = 0;
        self.invalidate_result();
    }

    pub(crate) fn reset(&mut self) {
        self.items.clear();
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
        self.items.remove(&index);
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
