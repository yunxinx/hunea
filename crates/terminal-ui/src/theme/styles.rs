use ratatui::{
    style::Style,
    text::Line,
    widgets::{Block, BorderType, Padding},
};

use super::TerminalPalette;

/// `SurfaceHalf` 表示用半块字符模拟 surface 的哪一半。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceHalf {
    Upper,
    Lower,
}

/// `primary_text_style` 返回主体文字样式。
pub fn primary_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.main)
}

/// `muted_text_style` 返回弱化正文样式。
pub fn muted_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.muted)
}

/// `secondary_text_style` 返回辅助信息样式。
pub fn secondary_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.secondary)
}

/// `tertiary_text_style` 返回更弱化的辅助文字样式。
pub fn tertiary_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.tertiary)
}

/// `accent_text_style` 返回用于当前选择与面板强调线的样式。
pub fn accent_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.accent)
}

/// `command_accent_text_style` 返回用于斜杠菜单当前命令的样式。
pub fn command_accent_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.command_accent)
}

/// `system_error_text_style` 返回运行时 system message 的错误文字样式。
pub fn system_error_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.system_error)
}

/// `quote_text_style` 返回 Markdown 引用块的文字样式。
pub fn quote_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new().italic(), palette.quote)
}

/// `table_header_text_style` 返回 Markdown 表头强调文字样式。
pub fn table_header_text_style(palette: TerminalPalette) -> Style {
    apply_foreground(Style::new(), palette.table_header)
}

/// `surface_text_style` 返回带弱化背景的正文样式。
pub fn surface_text_style(palette: TerminalPalette) -> Style {
    apply_surface(Style::new(), palette)
}

/// `surface_emphasis_style` 返回带弱化背景的强调正文样式。
pub fn surface_emphasis_style(palette: TerminalPalette) -> Style {
    apply_surface(Style::new().bold(), palette)
}

/// `surface_half_block_line` 用半块字符模拟半高 surface 装饰线。
pub(crate) fn surface_half_block_line(
    width: usize,
    palette: TerminalPalette,
    half: SurfaceHalf,
) -> Option<Line<'static>> {
    let surface = palette.surface?;
    let symbol = match half {
        SurfaceHalf::Upper => '▀',
        SurfaceHalf::Lower => '▄',
    };

    Some(Line::styled(
        symbol.to_string().repeat(width.max(1)),
        Style::new().fg(surface),
    ))
}

/// `surface_half_block_plain_line` 保持半高装饰线在文本语义上为空白。
pub(crate) fn surface_half_block_plain_line(width: usize) -> String {
    " ".repeat(width.max(1))
}

/// `subtle_rule_line` 返回全屏预览内部使用的弱分隔线。
pub(crate) fn subtle_rule_line(width: usize, palette: TerminalPalette) -> Line<'static> {
    Line::styled("╌".repeat(width.max(1)), tertiary_text_style(palette))
}

/// `panel_block` 返回用于启动欢迎块容器的统一边框样式。
pub fn panel_block(palette: TerminalPalette) -> Block<'static> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(2));

    if palette.uses_terminal_default_colors() {
        block
    } else {
        block.border_style(secondary_text_style(palette))
    }
}

fn apply_surface(style: Style, palette: TerminalPalette) -> Style {
    let style = apply_foreground(style, palette.main);

    match palette.surface {
        Some(surface) => style.bg(surface),
        None => style,
    }
}

fn apply_foreground(style: Style, color: ratatui::style::Color) -> Style {
    if color == ratatui::style::Color::Reset {
        style
    } else {
        style.fg(color)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::{
        accent_text_style, command_accent_text_style, muted_text_style, panel_block,
        primary_text_style, quote_text_style, secondary_text_style, surface_emphasis_style,
        surface_text_style, system_error_text_style, table_header_text_style, tertiary_text_style,
    };
    use crate::theme::{default_palette, terminal_default_palette};

    #[test]
    fn text_styles_use_semantic_palette_slots() {
        let palette = default_palette();

        assert_eq!(primary_text_style(palette).fg, Some(palette.main));
        assert_eq!(muted_text_style(palette).fg, Some(palette.muted));
        assert_eq!(secondary_text_style(palette).fg, Some(palette.secondary));
        assert_eq!(tertiary_text_style(palette).fg, Some(palette.tertiary));
        assert_eq!(accent_text_style(palette).fg, Some(palette.accent));
        assert_eq!(
            command_accent_text_style(palette).fg,
            Some(palette.command_accent)
        );
        assert_eq!(
            system_error_text_style(palette).fg,
            Some(palette.system_error)
        );
        assert_eq!(
            table_header_text_style(palette).fg,
            Some(palette.table_header)
        );
        assert_eq!(quote_text_style(palette).fg, Some(palette.quote));
        assert!(
            quote_text_style(palette)
                .add_modifier
                .contains(ratatui::style::Modifier::ITALIC)
        );
        assert_eq!(surface_text_style(palette).fg, Some(palette.main));
        assert_eq!(surface_text_style(palette).bg, palette.surface);
        assert_eq!(surface_emphasis_style(palette).fg, Some(palette.main));
        assert_eq!(surface_emphasis_style(palette).bg, palette.surface);
        assert_eq!(
            super::surface_half_block_line(3, palette, super::SurfaceHalf::Lower)
                .expect("default palette should have a surface color")
                .style
                .fg,
            palette.surface
        );
        assert!(
            surface_emphasis_style(palette)
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
    }

    #[test]
    fn panel_block_uses_rounded_border_padding_and_secondary_color() {
        let palette = default_palette();
        let block = panel_block(palette);
        let area = Rect::new(0, 0, 10, 4);
        let mut buffer = Buffer::empty(area);

        assert_eq!(block.inner(area), Rect::new(3, 1, 4, 2));
        block.render(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "╭");
        assert_eq!(buffer[(9, 0)].symbol(), "╮");
        assert_eq!(buffer[(0, 3)].symbol(), "╰");
        assert_eq!(buffer[(9, 3)].symbol(), "╯");
        assert_eq!(buffer[(0, 0)].fg, palette.secondary);
        assert_eq!(buffer[(9, 0)].fg, palette.secondary);
    }

    #[test]
    fn panel_block_leaves_border_colors_reset_for_terminal_defaults() {
        let block = panel_block(terminal_default_palette());
        let area = Rect::new(0, 0, 10, 4);
        let mut buffer = Buffer::empty(area);

        block.render(area, &mut buffer);

        assert_eq!(buffer[(0, 0)].fg, ratatui::style::Color::Reset);
        assert_eq!(buffer[(9, 0)].fg, ratatui::style::Color::Reset);
    }
}
