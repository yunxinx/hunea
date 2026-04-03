use ratatui::text::Line;

use super::render_state::{ItemLineAnchor, RenderResult};

/// `CachedRenderBlock` 缓存单个 transcript item 在某个宽度下的屏幕渲染结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct CachedRenderBlock {
    pub(crate) item_index: usize,
    pub(crate) cache_key: u64,
    pub(crate) width: u16,
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<ItemLineAnchor>,
    pub(crate) valid: bool,
}

/// `ScreenRenderCache` 管理 transcript 的 item 级缓存与整体结果缓存。
#[derive(Debug, Clone, Default)]
pub(crate) struct ScreenRenderCache {
    pub(crate) items: Vec<CachedRenderBlock>,
    pub(crate) result: RenderResult,
    pub(crate) width: u16,
    pub(crate) gap: usize,
    pub(crate) item_count: usize,
    pub(crate) valid: bool,
}

impl ScreenRenderCache {
    pub(crate) fn ensure_item_count(&mut self, count: usize) {
        if count <= self.items.len() {
            self.items.truncate(count);
            return;
        }

        self.items.extend(
            std::iter::repeat_with(CachedRenderBlock::default).take(count - self.items.len()),
        );
    }

    pub(crate) fn invalidate_result(&mut self) {
        self.result = RenderResult::default();
        self.valid = false;
    }

    pub(crate) fn invalidate_all(&mut self) {
        for item in &mut self.items {
            *item = CachedRenderBlock::default();
        }
        self.invalidate_result();
    }

    pub(crate) fn reset(&mut self) {
        self.items.clear();
        self.invalidate_result();
        self.width = 0;
        self.gap = 0;
        self.item_count = 0;
    }

    pub(crate) fn can_reuse_result(&self, width: u16, gap: usize, item_count: usize) -> bool {
        self.valid && self.width == width && self.gap == gap && self.item_count == item_count
    }

    pub(crate) fn can_extend_result(&self, width: u16, gap: usize, item_count: usize) -> bool {
        self.valid && self.width == width && self.gap == gap && self.item_count < item_count
    }

    pub(crate) fn store_result(
        &mut self,
        width: u16,
        gap: usize,
        item_count: usize,
        result: RenderResult,
    ) {
        self.result = result;
        self.width = width;
        self.gap = gap;
        self.item_count = item_count;
        self.valid = true;
    }
}
