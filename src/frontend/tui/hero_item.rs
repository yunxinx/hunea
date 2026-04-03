use ratatui::text::Line;

use super::{
    HeroOptions,
    hero::{
        render_hero_buffer_with_palette, render_hero_lines_with_palette,
        render_hero_plain_lines_with_palette,
    },
    theme::TerminalPalette,
};

/// `HeroItem` 表示 transcript 的开场项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeroItem {
    options: HeroOptions,
}

impl HeroItem {
    /// `new` 创建一条 hero 项。
    pub fn new(options: HeroOptions) -> Self {
        Self { options }
    }

    /// `render_lines` 将 hero 渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        adjusted_options(&self.options, width, palette)
            .map(|options| render_hero_lines_with_palette(&options, palette))
            .unwrap_or_else(|| render_hero_lines_with_palette(&self.options, palette))
    }

    /// `render_plain` 返回用于退出后打印的纯文本内容。
    pub fn render_plain(&self, width: u16, palette: TerminalPalette) -> String {
        adjusted_options(&self.options, width, palette)
            .map(|options| render_hero_plain_lines_with_palette(&options, palette))
            .unwrap_or_else(|| render_hero_plain_lines_with_palette(&self.options, palette))
            .join("\n")
    }
}

fn adjusted_options(
    options: &HeroOptions,
    width: u16,
    palette: TerminalPalette,
) -> Option<HeroOptions> {
    if width == 0 {
        return None;
    }

    let natural_width = render_hero_buffer_with_palette(options, palette).area.width;
    if natural_width <= width {
        return None;
    }

    Some(HeroOptions {
        width: width.saturating_sub(6).max(1),
        ..options.clone()
    })
}
