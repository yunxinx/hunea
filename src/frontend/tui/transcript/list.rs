use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

use ratatui::text::Line;

use super::{
    DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchor, LineAnchorKind, RenderResult,
    ViewportRenderResult, cache::CachedRenderBlock, cache::ScreenRenderCache, new_render_result,
};
use crate::frontend::tui::{
    HeroOptions, Sender, StyleMode, hero_item::HeroItem, message_item::MessageItem,
    styled_text::line_to_plain_text, theme::TerminalPalette,
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
    items: Vec<TranscriptItem>,
    gap: usize,
    width: u16,
    palette: TerminalPalette,
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
            items: Vec::new(),
            gap: 1,
            width: DEFAULT_RENDER_WIDTH as u16,
            palette,
            screen_cache: ScreenRenderCache::default(),
        }
    }

    /// `set_gap` 设置项与项之间的空行数。
    pub(crate) fn set_gap(&mut self, gap: usize) {
        if self.gap == gap {
            return;
        }

        self.gap = gap;
        self.screen_cache.invalidate_result();
    }

    /// `set_width` 设置 transcript 的可用宽度。
    pub(crate) fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
    }

    /// `set_palette` 刷新 transcript 使用的配色。
    pub(crate) fn set_palette(&mut self, palette: TerminalPalette) {
        if self.palette == palette {
            return;
        }

        self.palette = palette;
        self.screen_cache.invalidate_all();
    }

    /// `append_hero` 追加一条 hero 项。
    pub(crate) fn append_hero(&mut self, options: HeroOptions) {
        self.items
            .push(TranscriptItem::Hero(HeroItem::new(options)));
        self.screen_cache.ensure_item_count(self.items.len());
    }

    /// `append_message` 追加一条消息项。
    #[cfg(test)]
    pub(crate) fn append_message(&mut self, sender: Sender, content: impl Into<String>) {
        self.items
            .push(TranscriptItem::Message(MessageItem::new(sender, content)));
        self.screen_cache.ensure_item_count(self.items.len());
    }

    /// `append_message_with_style_mode` 追加一条带样式模式的消息项。
    pub(crate) fn append_message_with_style_mode(
        &mut self,
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
    ) {
        self.items
            .push(TranscriptItem::Message(MessageItem::new_with_style_mode(
                sender, content, style_mode,
            )));
        self.screen_cache.ensure_item_count(self.items.len());
    }

    /// `len` 返回 transcript 项数量。
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// `clear` 清空 transcript。
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.screen_cache.reset();
    }

    /// `item` 返回指定索引的 transcript 项。
    #[allow(dead_code)]
    pub(crate) fn item(&self, index: usize) -> Option<&TranscriptItem> {
        self.items.get(index)
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
    pub(crate) fn render(&mut self) -> RenderResult {
        let width = self.render_width();
        if self.can_reuse_cached_render_result(width) {
            return self.screen_cache.result.clone();
        }
        let (blocks, first_changed_index) = self.render_screen_blocks(width);
        if first_changed_index.is_none()
            && self
                .screen_cache
                .can_reuse_result(width, self.gap, self.items.len())
        {
            return self.screen_cache.result.clone();
        }
        if let Some(first_changed_index) = first_changed_index
            && self
                .screen_cache
                .can_extend_result(width, self.gap, self.items.len())
            && first_changed_index >= self.screen_cache.item_count
        {
            let result = self.extend_render_result(width, &blocks);
            self.screen_cache
                .store_result(width, self.gap, self.items.len(), result.clone());
            return result;
        }

        if blocks.is_empty() {
            let result = RenderResult::default();
            self.screen_cache
                .store_result(width, self.gap, self.items.len(), result.clone());
            return result;
        }

        let mut lines = Vec::with_capacity(total_block_lines(&blocks, self.gap));
        let mut plain_lines = Vec::with_capacity(total_block_lines(&blocks, self.gap));
        let mut line_anchors = Vec::with_capacity(total_block_lines(&blocks, self.gap));

        for (index, block) in blocks.iter().enumerate() {
            lines.extend(block.lines.iter().cloned());
            plain_lines.extend(block.plain_lines.iter().cloned());
            line_anchors.extend(line_anchors_for_block(block));

            if index + 1 < blocks.len() && self.gap > 0 {
                for gap_offset in 0..self.gap {
                    lines.push(Line::raw(""));
                    plain_lines.push(String::new());
                    line_anchors.push(gap_line_anchor_for_block(block.item_index, gap_offset));
                }
            }
        }

        let result = new_render_result(lines, plain_lines, line_anchors);
        self.screen_cache
            .store_result(width, self.gap, self.items.len(), result.clone());
        result
    }

    /// `render_viewport` 返回 transcript 的可视切片。
    #[allow(dead_code)]
    pub(crate) fn render_viewport(&mut self, offset: usize, height: usize) -> ViewportRenderResult {
        self.render().viewport(offset, height)
    }

    fn render_width(&self) -> u16 {
        self.width.max(1)
    }

    /// `can_reuse_cached_render_result` 判断是否可以直接复用整份 RenderResult。
    /// 只有 item 级缓存键、宽度与数量都保持一致时，才允许跳过 block 重建。
    fn can_reuse_cached_render_result(&self, width: u16) -> bool {
        if !self
            .screen_cache
            .can_reuse_result(width, self.gap, self.items.len())
        {
            return false;
        }

        for (index, item) in self.items.iter().enumerate() {
            let cached = &self.screen_cache.items[index];
            if !cached.valid || cached.width != width || cached.cache_key != item.render_cache_key()
            {
                return false;
            }
        }

        true
    }

    fn render_screen_blocks(&mut self, width: u16) -> (Vec<CachedRenderBlock>, Option<usize>) {
        if self.items.is_empty() {
            return (Vec::new(), None);
        }

        self.screen_cache.ensure_item_count(self.items.len());

        let mut blocks = Vec::with_capacity(self.items.len());
        let mut first_changed_index = None;

        for index in 0..self.items.len() {
            let (block, changed) = self.render_screen_block(index, width);
            if changed && first_changed_index.is_none() {
                first_changed_index = Some(index);
            }
            if !block.lines.is_empty() {
                blocks.push(block);
            }
        }

        (blocks, first_changed_index)
    }

    fn render_screen_block(&mut self, index: usize, width: u16) -> (CachedRenderBlock, bool) {
        let cache_key = self.items[index].render_cache_key();
        let cached = &mut self.screen_cache.items[index];
        if cached.valid && cached.width == width && cached.cache_key == cache_key {
            return (cached.clone(), false);
        }

        let lines = self.items[index].render_lines(width, self.palette);
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();
        let mut anchors = self.items[index].render_line_anchors(width, self.palette);
        if anchors.len() != lines.len() {
            anchors = fallback_rendered_line_anchors(lines.len());
        }

        let block = CachedRenderBlock {
            item_index: index,
            cache_key,
            width,
            lines,
            plain_lines,
            anchors,
            valid: true,
        };
        *cached = block.clone();
        (block, true)
    }

    fn extend_render_result(&self, width: u16, blocks: &[CachedRenderBlock]) -> RenderResult {
        let base = self.screen_cache.result.clone();
        let mut lines = base.lines.clone();
        let mut plain_lines = base.plain_lines.clone();
        let mut line_anchors = base.line_anchors.clone();

        let mut previous_visible_item_index = None;
        for block in blocks {
            if block.item_index >= self.screen_cache.item_count {
                break;
            }
            previous_visible_item_index = Some(block.item_index);
        }
        let mut previous_anchor_item_index = previous_visible_item_index;
        let mut has_rendered_content = !lines.is_empty();

        for block in blocks {
            if block.item_index < self.screen_cache.item_count {
                continue;
            }

            if has_rendered_content && self.gap > 0 {
                let gap_item_index = previous_anchor_item_index.unwrap_or(block.item_index);
                for gap_offset in 0..self.gap {
                    lines.push(Line::raw(""));
                    plain_lines.push(String::new());
                    line_anchors.push(gap_line_anchor_for_block(gap_item_index, gap_offset));
                }
            }

            lines.extend(block.lines.iter().cloned());
            plain_lines.extend(block.plain_lines.iter().cloned());
            line_anchors.extend(line_anchors_for_block(block));
            has_rendered_content = true;
            previous_anchor_item_index = Some(block.item_index);
        }

        let _ = width;
        new_render_result(lines, plain_lines, line_anchors)
    }
}

impl TranscriptItem {
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

    fn render_line_anchors(&self, width: u16, palette: TerminalPalette) -> Vec<ItemLineAnchor> {
        match self {
            Self::Hero(item) => item.render_line_anchors(width, palette),
            Self::Message(item) => item.render_line_anchors(width, palette),
        }
    }

    fn render_cache_key(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        match self {
            Self::Hero(item) => item.render_cache_key().hash(&mut hasher),
            Self::Message(item) => item.render_cache_key().hash(&mut hasher),
        }
        hasher.finish()
    }
}

fn fallback_rendered_line_anchors(line_count: usize) -> Vec<ItemLineAnchor> {
    (0..line_count)
        .map(|rendered_line| ItemLineAnchor {
            kind: LineAnchorKind::RenderedLine,
            rendered_line,
            ..ItemLineAnchor::default()
        })
        .collect()
}

fn line_anchors_for_block(block: &CachedRenderBlock) -> Vec<LineAnchor> {
    block
        .anchors
        .iter()
        .copied()
        .map(|item_anchor| LineAnchor {
            item_index: block.item_index,
            item_anchor,
        })
        .collect()
}

fn gap_line_anchor_for_block(item_index: usize, gap_offset: usize) -> LineAnchor {
    LineAnchor {
        item_index,
        item_anchor: ItemLineAnchor {
            kind: LineAnchorKind::ItemGap,
            gap_offset,
            ..ItemLineAnchor::default()
        },
    }
}

fn total_block_lines(blocks: &[CachedRenderBlock], gap: usize) -> usize {
    let total = blocks.iter().map(|block| block.lines.len()).sum::<usize>();
    if blocks.len() <= 1 || gap == 0 {
        return total;
    }

    total + (blocks.len() - 1) * gap
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;

    use super::*;
    use crate::frontend::tui::theme::default_palette;

    #[test]
    fn render_returns_content_lines_and_line_count() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![
            TranscriptItem::Message(MessageItem::new(Sender::Assistant, "one\ntwo")),
            TranscriptItem::Message(MessageItem::new(Sender::Assistant, "three")),
        ];

        let result = transcript.render();
        let rendered = result
            .lines
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
    fn render_append_path_keeps_gap_anchor_on_previous_item() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![TranscriptItem::Message(static_message("first"))];
        let _ = transcript.render();

        transcript
            .items
            .push(TranscriptItem::Message(static_message("second")));
        let result = transcript.render();

        assert_eq!(result.line_anchors.len(), 3);
        assert_eq!(result.line_anchors[1].item_index, 0);
        assert_eq!(
            result.line_anchors[1].item_anchor.kind,
            LineAnchorKind::ItemGap
        );
    }

    #[test]
    fn render_builds_gap_anchor_between_visible_blocks() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![
            TranscriptItem::Message(static_message("one")),
            TranscriptItem::Message(static_message("two")),
        ];

        let result = transcript.render();

        assert_eq!(result.line_anchors.len(), 3);
        assert_eq!(result.line_anchors[1].item_index, 0);
        assert_eq!(
            result.line_anchors[1].item_anchor.kind,
            LineAnchorKind::ItemGap
        );
        assert_eq!(result.line_anchors[2].item_index, 1);
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn render_perf_smoke_for_large_cached_transcript() {
        use std::hint::black_box;

        let mut transcript = Transcript::new(default_palette());
        transcript.set_width(72);

        for index in 0..64 {
            transcript.items.push(TranscriptItem::Message(static_message(&format!(
                "item {index:02}\nalpha beta gamma alpha beta gamma\ndelta epsilon zeta delta epsilon zeta"
            ))));
        }

        for _ in 0..128 {
            black_box(transcript.render());
        }
    }

    #[test]
    fn cached_render_result_can_be_reused_when_item_cache_keys_are_stable() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![TranscriptItem::Message(static_message("cached"))];

        let _ = transcript.render();

        assert!(transcript.can_reuse_cached_render_result(transcript.render_width()));
    }

    #[test]
    fn cached_render_result_becomes_stale_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![TranscriptItem::Message(static_message("one"))];

        let _ = transcript.render();
        transcript.items[0] = TranscriptItem::Message(static_message("two"));

        assert!(!transcript.can_reuse_cached_render_result(transcript.render_width()));
    }

    #[test]
    fn render_refreshes_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![TranscriptItem::Message(static_message("one"))];

        let first = transcript.render();
        assert_eq!(first.plain_lines, vec!["one"]);

        transcript.items[0] = TranscriptItem::Message(static_message("two"));

        let second = transcript.render();
        assert_eq!(second.plain_lines, vec!["two"]);
    }

    #[test]
    fn render_viewport_refreshes_after_item_content_changes() {
        let mut transcript = Transcript::new(default_palette());
        transcript.items = vec![TranscriptItem::Message(static_message("one\ntwo"))];

        let first = transcript.render_viewport(1, 1);
        assert_eq!(first.plain_lines, vec!["two"]);

        transcript.items[0] = TranscriptItem::Message(static_message("alpha\nbeta"));

        let second = transcript.render_viewport(1, 1);
        assert_eq!(second.plain_lines, vec!["beta"]);
    }

    fn static_message(content: &str) -> MessageItem {
        MessageItem::new(Sender::Assistant, content)
    }

    #[allow(dead_code)]
    fn styled_line(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }
}
