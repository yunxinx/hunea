//! Dedicated palette slots for context budget segment kinds.

use ratatui::style::Color;
use runtime_domain::context_budget::SegmentKind;

use super::summary::ContextBudgetCategoryKind;
use crate::theme::TerminalPalette;

const CONTEXT_BUDGET_SLOT_COUNT: usize = 6;
const TERMINAL_DEFAULT_CONTEXT_BUDGET_COLORS: [Color; CONTEXT_BUDGET_SLOT_COUNT] = [
    Color::Blue,
    Color::Yellow,
    Color::Green,
    Color::Red,
    Color::Magenta,
    Color::Cyan,
];

/// Maps a segment kind to a stable palette slot (extensible without changing heatmap logic).
pub(crate) fn context_budget_color_for_kind(kind: SegmentKind, palette: &TerminalPalette) -> Color {
    let slot = kind_palette_slot(kind);
    context_budget_slot_color(slot, palette)
}

pub(crate) fn context_budget_empty_color(palette: &TerminalPalette) -> Color {
    palette.tertiary
}

pub(crate) fn context_budget_color_for_category(
    kind: ContextBudgetCategoryKind,
    palette: &TerminalPalette,
) -> Color {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => {
            context_budget_color_for_kind(SegmentKind::System, palette)
        }
        ContextBudgetCategoryKind::ToolDefinitions => {
            context_budget_color_for_kind(SegmentKind::ToolDefinitions, palette)
        }
        ContextBudgetCategoryKind::Messages => {
            context_budget_color_for_kind(SegmentKind::AssistantMessage, palette)
        }
        ContextBudgetCategoryKind::FreeSpace => context_budget_empty_color(palette),
    }
}

fn kind_palette_slot(kind: SegmentKind) -> usize {
    match kind {
        SegmentKind::System => 0,
        SegmentKind::UserMessage => 1,
        SegmentKind::AssistantMessage => 2,
        SegmentKind::ToolResult => 3,
        SegmentKind::Reasoning => 4,
        SegmentKind::ToolDefinitions => 5,
    }
}

fn context_budget_slot_color(slot: usize, palette: &TerminalPalette) -> Color {
    let slot = slot % CONTEXT_BUDGET_SLOT_COUNT;
    if palette.uses_terminal_default_colors() {
        return TERMINAL_DEFAULT_CONTEXT_BUDGET_COLORS[slot];
    }

    let dark_background = color_brightness(palette.main) > 127.0;
    match (dark_background, slot) {
        (true, 0) => Color::Rgb(96, 165, 250),
        (true, 1) => Color::Rgb(251, 191, 36),
        (true, 2) => Color::Rgb(74, 222, 128),
        (true, 3) => Color::Rgb(248, 113, 113),
        (true, 4) => Color::Rgb(167, 139, 250),
        (true, 5) => Color::Rgb(34, 211, 238),
        (false, 0) => Color::Rgb(29, 78, 216),
        (false, 1) => Color::Rgb(180, 83, 9),
        (false, 2) => Color::Rgb(21, 128, 61),
        (false, 3) => Color::Rgb(185, 28, 28),
        (false, 4) => Color::Rgb(109, 40, 217),
        (false, 5) => Color::Rgb(8, 145, 178),
        _ => unreachable!("context budget slot must stay within 0..{CONTEXT_BUDGET_SLOT_COUNT}"),
    }
}

fn color_brightness(color: Color) -> f32 {
    match color {
        Color::Rgb(red, green, blue) => (red as f32 + green as f32 + blue as f32) / 3.0,
        Color::White | Color::Gray | Color::LightBlue | Color::LightCyan | Color::LightGreen => {
            255.0
        }
        Color::Black | Color::DarkGray => 0.0,
        _ => 127.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{default_palette, terminal_default_palette};

    #[test]
    fn distinct_kinds_map_to_colors_without_panic() {
        let palette = default_palette();
        for kind in [
            SegmentKind::System,
            SegmentKind::UserMessage,
            SegmentKind::AssistantMessage,
            SegmentKind::ToolDefinitions,
        ] {
            let _ = context_budget_color_for_kind(kind, &palette);
        }
    }

    #[test]
    fn terminal_default_palette_uses_ansi_safe_segment_colors() {
        let palette = terminal_default_palette();

        assert_eq!(
            context_budget_color_for_kind(SegmentKind::System, &palette),
            Color::Blue
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::UserMessage, &palette),
            Color::Yellow
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::AssistantMessage, &palette),
            Color::Green
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::ToolResult, &palette),
            Color::Red
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::Reasoning, &palette),
            Color::Magenta
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::ToolDefinitions, &palette),
            Color::Cyan
        );
        assert_eq!(context_budget_empty_color(&palette), Color::Reset);
    }
}
