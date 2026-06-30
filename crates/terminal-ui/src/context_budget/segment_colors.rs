//! Dedicated palette slots for context budget segment kinds.

use ratatui::style::Color;

use super::summary::ContextBudgetCategoryKind;
use crate::theme::{
    ContextBudgetColorSlot, TerminalPalette, context_budget_empty_color, context_budget_slot_color,
};

pub(crate) fn context_budget_color_for_category(
    kind: ContextBudgetCategoryKind,
    palette: &TerminalPalette,
) -> Color {
    match kind {
        ContextBudgetCategoryKind::SystemPrompt => {
            context_budget_slot_color(ContextBudgetColorSlot::SystemPrompt, palette)
        }
        ContextBudgetCategoryKind::ToolDefinitions => {
            context_budget_slot_color(ContextBudgetColorSlot::ToolDefinitions, palette)
        }
        ContextBudgetCategoryKind::Messages => {
            context_budget_slot_color(ContextBudgetColorSlot::Messages, palette)
        }
        ContextBudgetCategoryKind::FreeSpace => context_budget_empty_color(palette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{default_palette, palette_from_background, terminal_default_palette};

    #[test]
    fn distinct_categories_map_to_colors_without_panic() {
        let palette = default_palette();
        for kind in [
            ContextBudgetCategoryKind::SystemPrompt,
            ContextBudgetCategoryKind::Messages,
            ContextBudgetCategoryKind::ToolDefinitions,
            ContextBudgetCategoryKind::FreeSpace,
        ] {
            let _ = context_budget_color_for_category(kind, &palette);
        }
    }

    #[test]
    fn terminal_default_palette_uses_ansi_safe_category_colors() {
        let palette = terminal_default_palette();

        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::SystemPrompt, &palette),
            Color::Blue
        );
        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::Messages, &palette),
            Color::Green
        );
        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::ToolDefinitions, &palette),
            Color::Cyan
        );
        assert_eq!(context_budget_empty_color(&palette), Color::Reset);
    }

    #[test]
    fn explicit_dark_palette_uses_bright_category_colors() {
        let palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));

        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::SystemPrompt, &palette),
            Color::Rgb(96, 165, 250)
        );
        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::Messages, &palette),
            Color::Rgb(74, 222, 128)
        );
    }

    #[test]
    fn explicit_light_palette_uses_deep_category_colors() {
        let palette = palette_from_background(false, Some(Color::Rgb(240, 240, 240)));

        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::SystemPrompt, &palette),
            Color::Rgb(29, 78, 216)
        );
        assert_eq!(
            context_budget_color_for_category(ContextBudgetCategoryKind::Messages, &palette),
            Color::Rgb(21, 128, 61)
        );
    }
}
