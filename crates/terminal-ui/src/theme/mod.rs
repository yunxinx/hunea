mod page_rule;
mod palette;
mod styles;

pub(crate) use page_rule::{build_labeled_rule, build_page_rule};
pub(crate) use palette::{
    ContextBudgetColorSlot, TerminalBackgroundColor, context_budget_empty_color,
    context_budget_slot_color, palette_detection_from_background,
};
pub use palette::{
    PaletteDetection, TerminalColorCapability, TerminalPalette, default_palette, detect_palette,
    palette_from_background, terminal_default_palette, try_detect_palette,
};
pub(crate) use styles::{
    SurfaceHalf, subtle_rule_line, surface_half_block_line, surface_half_block_plain_line,
};
pub use styles::{
    accent_rule_line, accent_text_style, approval_rejected_text_style, command_accent_text_style,
    muted_text_style, panel_block, primary_text_style, quote_text_style, secondary_text_style,
    surface_emphasis_style, surface_text_style, system_error_text_style, table_header_text_style,
    tertiary_text_style,
};
