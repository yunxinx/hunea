use std::rc::Rc;

use ratatui::text::Line;

use super::{
    DEFAULT_RENDER_WIDTH, ItemLineAnchor, LineAnchorKind, RenderResult, ViewportRenderResult,
    cache::CachedRenderBlock, cache::ScreenRenderCache, new_render_result_with_append_start,
    render_state::RenderItemSummary,
};
use crate::frontend::tui::{
    HeroOptions, Sender, StyleMode,
    hero_item::HeroItem,
    message_item::MessageItem,
    selection::{SelectableLineRange, normalize_transcript_selectable_range},
    styled_text::line_to_plain_text,
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
            screen_cache: ScreenRenderCache::default(),
        }
    }

    /// `set_gap` 设置项与项之间的空行数。
    pub(crate) fn set_gap(&mut self, gap: usize) {
        if self.gap == gap {
            return;
        }

        self.gap = gap;
        self.screen_cache.mark_dirty_from(0);
    }

    /// `set_width` 设置 transcript 的可用宽度。
    pub(crate) fn set_width(&mut self, width: u16) {
        let width = width.max(1);
        if self.width == width {
            return;
        }

        self.width = width;
        self.screen_cache.invalidate_all();
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
    pub(crate) fn render(&mut self) -> Rc<RenderResult> {
        let width = self.render_width();
        if self
            .screen_cache
            .can_reuse_result(width, self.gap, self.items.len(), self.items_version)
        {
            return Rc::clone(&self.screen_cache.result);
        }
        if self.items.is_empty() {
            let result = Rc::new(RenderResult::default());
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
        let result = Rc::new(self.build_render_result(width, dirty_from, append_start_line));
        self.screen_cache.store_result(
            width,
            self.gap,
            self.items.len(),
            self.items_version,
            Rc::clone(&result),
        );
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
    ) -> RenderResult {
        let previous = Rc::clone(&self.screen_cache.result);
        let mut items = Vec::with_capacity(self.items.len());
        let mut total_lines = 0;
        let mut previous_visible_item_index = None;

        if dirty_from > 0 {
            for summary in previous.items.iter() {
                if summary.item_index >= dirty_from {
                    break;
                }
                total_lines = summary.start_line + summary.total_line_count;
                previous_visible_item_index = Some(summary.item_index);
                items.push(summary.clone());
            }
        }

        for index in dirty_from..self.items.len() {
            let block = self.render_screen_block(index, width);
            if block.lines.is_empty() {
                continue;
            }

            let gap_before = usize::from(previous_visible_item_index.is_some()) * self.gap;
            let summary = RenderItemSummary {
                item_index: index,
                start_line: total_lines,
                gap_before,
                content_line_count: block.lines.len(),
                total_line_count: gap_before + block.lines.len(),
                content_char_len: block.plain_text_char_len,
                gap_owner_item_index: previous_visible_item_index,
                block,
            };
            total_lines += summary.total_line_count;
            previous_visible_item_index = Some(index);
            items.push(summary);
        }

        new_render_result_with_append_start(items, append_start_line)
    }

    fn render_screen_block(&mut self, index: usize, width: u16) -> Rc<CachedRenderBlock> {
        let cache_key = self.items[index].render_cache_key();
        if let Some(cached) = self.screen_cache.items.get(&index)
            && cached.width == width
            && cached.cache_key == cache_key
        {
            return Rc::clone(cached);
        }

        let lines = self.items[index].render_lines(width, self.palette);
        let plain_lines = lines.iter().map(line_to_plain_text).collect::<Vec<_>>();
        let mut anchors = self.items[index].render_line_anchors(width, self.palette);
        if anchors.len() != lines.len() {
            anchors = fallback_rendered_line_anchors(lines.len());
        }
        let block = Rc::new(CachedRenderBlock {
            cache_key,
            width,
            plain_text_char_len: plain_lines.iter().map(String::len).sum(),
            lines: Rc::new(lines),
            plain_lines: Rc::new(plain_lines),
            anchors: Rc::new(anchors),
        });
        self.screen_cache.items.insert(index, Rc::clone(&block));
        block
    }

    fn push_item(&mut self, item: TranscriptItem) {
        let len_before_append = self.items.len();
        Rc::make_mut(&mut self.items).push(Rc::new(item));
        self.items_version = self.items_version.saturating_add(1);
        self.screen_cache.mark_dirty_from(len_before_append);
    }

    #[cfg(test)]
    fn replace_item_for_test(&mut self, index: usize, item: TranscriptItem) {
        Rc::make_mut(&mut self.items)[index] = Rc::new(item);
        self.items_version = self.items_version.saturating_add(1);
        self.screen_cache.clear_item(index);
    }

    #[cfg(test)]
    pub(crate) fn dirty_from_for_test(&self) -> usize {
        self.screen_cache.dirty_from
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

    fn render_cache_key(&self) -> u64 {
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

fn fallback_rendered_line_anchors(line_count: usize) -> Vec<ItemLineAnchor> {
    (0..line_count)
        .map(|rendered_line| ItemLineAnchor {
            kind: LineAnchorKind::RenderedLine,
            rendered_line,
            ..ItemLineAnchor::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::text::Span;

    use super::*;
    use crate::frontend::tui::{
        StyleMode,
        message_item::{
            message_item_render_cache_key_call_count,
            reset_message_item_render_cache_key_call_count,
        },
        theme::default_palette,
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
            transcript.screen_cache.items.len(),
            0,
            "append should not grow dense render cache slots before any render happens"
        );
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

    fn static_message(content: &str) -> MessageItem {
        MessageItem::new(Sender::Assistant, content)
    }

    #[allow(dead_code)]
    fn styled_line(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }
}
