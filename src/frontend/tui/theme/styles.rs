use ratatui::{
    style::{Modifier, Style},
    widgets::{Block, BorderType, Padding},
};

use super::TerminalPalette;

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

/// `surface_text_style` 返回带弱化背景的正文样式。
pub fn surface_text_style(palette: TerminalPalette) -> Style {
    apply_surface(Style::new(), palette)
}

/// `surface_emphasis_style` 返回带弱化背景的强调正文样式。
pub fn surface_emphasis_style(palette: TerminalPalette) -> Style {
    apply_surface(Style::new().add_modifier(Modifier::BOLD), palette)
}

/// `panel_block` 返回用于 hero 容器的统一边框样式。
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
    use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, widgets::Widget};

    use super::{
        muted_text_style, panel_block, primary_text_style, secondary_text_style,
        surface_emphasis_style, surface_text_style,
    };
    use crate::frontend::tui::theme::{default_palette, terminal_default_palette};

    #[test]
    fn text_styles_use_semantic_palette_slots() {
        let palette = default_palette();

        assert_eq!(primary_text_style(palette).fg, Some(palette.main));
        assert_eq!(muted_text_style(palette).fg, Some(palette.muted));
        assert_eq!(secondary_text_style(palette).fg, Some(palette.secondary));
        assert_eq!(surface_text_style(palette).fg, Some(palette.main));
        assert_eq!(surface_text_style(palette).bg, palette.surface);

        let emphasis = surface_emphasis_style(palette);
        assert_eq!(emphasis.fg, Some(palette.main));
        assert_eq!(emphasis.bg, palette.surface);
        assert!(emphasis.add_modifier.contains(Modifier::BOLD));
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
