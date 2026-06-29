//! Context budget view (`/context`) inline panel.

mod heatmap;
mod input;
mod layout;
mod legend;
mod render;
mod segment_colors;
mod state;
mod summary;

pub(super) const CONTEXT_BUDGET_SECTION_GAP_ROWS: usize = 1;
pub(super) const CONTEXT_BUDGET_PANEL_INSET_WIDTH: u16 = 2;
pub(super) const CONTEXT_BUDGET_COLUMN_GAP: u16 = 3;
pub(super) const CONTEXT_BUDGET_HEATMAP_WIDTH: u16 = 20;
pub(super) const CONTEXT_BUDGET_MIN_LEGEND_WIDTH: u16 = 20;
pub(super) const CONTEXT_BUDGET_HEATMAP_CELL_WIDTH: usize = 2;
pub(super) const CONTEXT_BUDGET_HEATMAP_GRID_COLUMNS: usize = 10;
pub(super) const CONTEXT_BUDGET_HEATMAP_GRID_ROWS: usize = 10;
pub(super) const CONTEXT_BUDGET_LEGEND_SWATCH_WIDTH: usize = 1;
pub(super) const CONTEXT_BUDGET_LEGEND_SWATCH_GAP: usize = 1;

pub(crate) use render::ContextBudgetRenderResult;
pub(crate) use state::ContextBudgetState;

#[cfg(test)]
mod tests;
