mod palette;
mod styles;

pub use palette::{
    PaletteDetection, TerminalPalette, default_palette, detect_palette, palette_from_background,
    terminal_default_palette, try_detect_palette,
};
pub use styles::{
    muted_text_style, panel_block, primary_text_style, secondary_text_style,
    surface_emphasis_style, surface_text_style,
};
