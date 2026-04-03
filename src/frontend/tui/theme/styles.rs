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
