mod palette;
mod styles;

pub use palette::{
    PaletteDetection, TerminalPalette, default_palette, detect_palette, palette_from_background,
    terminal_default_palette, try_detect_palette,
};
pub(crate) use palette::{TerminalBackgroundColor, palette_detection_from_background};
pub(crate) use styles::{SurfaceHalf, surface_half_block_line, surface_half_block_plain_line};
pub use styles::{
    accent_text_style, command_accent_text_style, muted_text_style, panel_block,
    primary_text_style, quote_text_style, secondary_text_style, surface_emphasis_style,
    surface_text_style, system_error_text_style, table_header_text_style, tertiary_text_style,
};
