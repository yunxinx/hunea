//! Dedicated palette slots for context budget segment kinds.

use ratatui::style::Color;
use runtime_domain::context_budget::SegmentKind;

use crate::theme::TerminalPalette;

const CONTEXT_BUDGET_SLOT_COUNT: usize = 8;

/// Maps a segment kind to a stable palette slot (extensible without changing heatmap logic).
pub(crate) fn context_budget_color_for_kind(kind: SegmentKind, palette: &TerminalPalette) -> Color {
    let slot = kind_palette_slot(kind);
    context_budget_slot_color(slot, palette)
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
    // Use distinct hues from secondary/tertiary/surface family — not error or command accent.
    match slot {
        0 => palette.secondary,
        1 => palette.tertiary,
        2 => palette.main,
        3 => palette.table_header,
        4 => palette.quote,
        5 => palette.muted,
        6 => palette.accent,
        7 => palette.command_accent,
        _ => palette.secondary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;

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
}
