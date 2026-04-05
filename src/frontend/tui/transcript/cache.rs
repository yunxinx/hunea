use std::{collections::HashMap, rc::Rc};

use ratatui::text::Line;

use super::render_state::{ItemLineAnchor, RenderResult};

/// `CachedRenderBlock` 缓存单个 transcript item 在某个宽度下的屏幕渲染结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct CachedRenderBlock {
    pub(crate) cache_key: u64,
    pub(crate) width: u16,
    pub(crate) lines: Rc<Vec<Line<'static>>>,
    pub(crate) plain_lines: Rc<Vec<String>>,
    pub(crate) anchors: Rc<Vec<ItemLineAnchor>>,
    pub(crate) plain_text_char_len: usize,
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
