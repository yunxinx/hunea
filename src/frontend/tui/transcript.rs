use ratatui::text::Line;

use super::{
    HeroOptions, Sender, hero_item::HeroItem, message_item::MessageItem, theme::TerminalPalette,
};

/// `TranscriptItem` 表示 transcript 中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptItem {
    Hero(HeroItem),
    Message(MessageItem),
}

/// `Transcript` 管理一组 document-flow 列表项及其布局间距。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    items: Vec<TranscriptItem>,
    gap: u16,
    width: u16,
    palette: TerminalPalette,
}

impl Transcript {
    /// `new` 创建一个空 transcript。
    pub fn new(palette: TerminalPalette) -> Self {
        Self {
            items: Vec::new(),
            gap: 1,
            width: 80,
            palette,
        }
    }

    /// `set_gap` 设置项与项之间的空行数。
    pub fn set_gap(&mut self, gap: u16) {
        self.gap = gap;
    }

    /// `set_width` 设置可用宽度。
    pub fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
    }

    /// `set_palette` 刷新 transcript 使用的配色。
    pub fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
    }

    /// `append_hero` 添加开场 hero 项。
    pub fn append_hero(&mut self, options: HeroOptions) {
        self.items
            .push(TranscriptItem::Hero(HeroItem::new(options)));
    }

    /// `append_message` 添加一条对话消息。
    pub fn append_message(&mut self, sender: Sender, content: impl Into<String>) {
        self.items
            .push(TranscriptItem::Message(MessageItem::new(sender, content)));
    }

    /// `render_lines` 将全部项渲染成带样式的文本行。
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        for (index, item) in self.items.iter().enumerate() {
            lines.extend(item.render_lines(self.width, self.palette));

            if index + 1 < self.items.len() {
                for _ in 0..self.gap {
                    lines.push(Line::raw(""));
                }
            }
        }

        lines
    }

    /// `plain_items` 返回退出后打印所需的纯文本项列表。
    pub fn plain_items(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|item| item.render_plain(self.width, self.palette))
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
