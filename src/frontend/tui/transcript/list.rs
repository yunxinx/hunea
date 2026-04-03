use ratatui::text::Line;

use super::DEFAULT_RENDER_WIDTH;
use crate::frontend::tui::{
    HeroOptions, Sender, hero_item::HeroItem, message_item::MessageItem, theme::TerminalPalette,
};

/// `RenderResult` 表示 transcript 在当前宽度下的稳定渲染结果。
#[derive(Debug, Clone, Default)]
pub(crate) struct RenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) line_count: usize,
}

/// `TranscriptItem` 表示 transcript 中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TranscriptItem {
    Hero(HeroItem),
    Message(MessageItem),
}

/// `Transcript` 管理 document-flow 顺序、宽度与项间距。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transcript {
    items: Vec<TranscriptItem>,
    gap: usize,
    width: u16,
    palette: TerminalPalette,
}

impl Transcript {
    /// `new` 创建一个空 transcript。
    pub(crate) fn new(palette: TerminalPalette) -> Self {
        Self {
            items: Vec::new(),
            gap: 1,
            width: DEFAULT_RENDER_WIDTH as u16,
            palette,
        }
    }

    /// `set_gap` 设置项与项之间的空行数。
    pub(crate) fn set_gap(&mut self, gap: usize) {
        self.gap = gap;
    }

    /// `set_width` 设置 transcript 的可用宽度。
    pub(crate) fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
    }

    /// `set_palette` 刷新 transcript 使用的配色。
    pub(crate) fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
    }

    /// `append_hero` 追加一条 hero 项。
    pub(crate) fn append_hero(&mut self, options: HeroOptions) {
        self.items
            .push(TranscriptItem::Hero(HeroItem::new(options)));
    }

    /// `append_message` 追加一条消息项。
    pub(crate) fn append_message(&mut self, sender: Sender, content: impl Into<String>) {
        self.items
            .push(TranscriptItem::Message(MessageItem::new(sender, content)));
    }

    /// `len` 返回当前 transcript 项数量。
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// `clear` 清空 transcript。
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    /// `item` 返回指定索引的 transcript 项。
    #[allow(dead_code)]
    pub(crate) fn item(&self, index: usize) -> Option<&TranscriptItem> {
        self.items.get(index)
    }

    /// `plain_items` 返回适用于退出后打印的文本项。
    pub(crate) fn plain_items(&self) -> Vec<String> {
        let width = self.render_width();

        self.items
            .iter()
            .map(|item| item.render_plain(width, self.palette))
            .filter(|item| !item.is_empty())
            .collect()
    }

    /// `render` 渲染整个 transcript，并缓存稳定的显式行数。
    pub(crate) fn render(&self) -> RenderResult {
        let rendered_items = self.rendered_items();
        if rendered_items.is_empty() {
            return RenderResult::default();
        }

        let mut lines = Vec::new();
        let item_count = rendered_items.len();
        for (index, item_lines) in rendered_items.into_iter().enumerate() {
            lines.extend(item_lines);

            if index + 1 < item_count && self.gap > 0 {
                for _ in 0..self.gap {
                    lines.push(Line::raw(""));
                }
            }
        }

        let line_count = lines.len();
        RenderResult { lines, line_count }
    }

    fn render_width(&self) -> u16 {
        self.width.max(1)
    }

    fn rendered_items(&self) -> Vec<Vec<Line<'static>>> {
        let width = self.render_width();

        self.items
            .iter()
            .map(|item| item.render_lines(width, self.palette))
            .filter(|item| !item.is_empty())
            .collect()
    }
}

impl TranscriptItem {
    fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self {
            Self::Hero(item) => item.render_lines(width, palette),
            Self::Message(item) => item.render_lines(width, palette),
        }
    }

    fn render_plain(&self, width: u16, palette: TerminalPalette) -> String {
        match self {
            Self::Hero(item) => item.render_plain(width, palette),
            Self::Message(item) => item.render_plain(width),
        }
    }
}

#[cfg(test)]
mod tests {
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

        assert_eq!(rendered, vec!["  one  ", "  two  ", "", "  three  "]);
        assert_eq!(result.line_count, 4);
    }
}
