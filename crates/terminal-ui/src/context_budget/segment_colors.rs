//! Dedicated palette slots for context budget segment kinds.

use ratatui::style::Color;
use runtime_domain::context_budget::SegmentKind;

use super::summary::ContextBudgetCategoryKind;
use crate::theme::{
    ContextBudgetColorSlot, TerminalPalette, context_budget_empty_color, context_budget_slot_color,
};

/// Maps a segment kind to a stable palette slot (extensible without changing heatmap logic).
pub(crate) fn context_budget_color_for_kind(kind: SegmentKind, palette: &TerminalPalette) -> Color {
    context_budget_slot_color(kind_color_slot(kind), palette)
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

fn kind_color_slot(kind: SegmentKind) -> ContextBudgetColorSlot {
    match kind {
        SegmentKind::System => ContextBudgetColorSlot::System,
        SegmentKind::UserMessage => ContextBudgetColorSlot::User,
        SegmentKind::AssistantMessage => ContextBudgetColorSlot::Assistant,
        SegmentKind::ToolResult => ContextBudgetColorSlot::ToolResult,
        SegmentKind::Reasoning => ContextBudgetColorSlot::Reasoning,
        SegmentKind::ToolDefinitions => ContextBudgetColorSlot::ToolDefinitions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{default_palette, palette_from_background, terminal_default_palette};

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

    #[test]
    fn explicit_dark_palette_uses_bright_segment_colors() {
        let palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));

        assert_eq!(
            context_budget_color_for_kind(SegmentKind::System, &palette),
            Color::Rgb(96, 165, 250)
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::AssistantMessage, &palette),
            Color::Rgb(74, 222, 128)
        );
    }

    #[test]
    fn explicit_light_palette_uses_deep_segment_colors() {
        let palette = palette_from_background(false, Some(Color::Rgb(240, 240, 240)));

        assert_eq!(
            context_budget_color_for_kind(SegmentKind::System, &palette),
            Color::Rgb(29, 78, 216)
        );
        assert_eq!(
            context_budget_color_for_kind(SegmentKind::AssistantMessage, &palette),
            Color::Rgb(21, 128, 61)
        );
    }
}
