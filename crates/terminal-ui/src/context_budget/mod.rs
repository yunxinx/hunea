//! Context budget view (`/context`) inline panel.

mod heatmap;
mod input;
mod layout;
mod legend;
mod payload;
mod render;
mod segment_colors;
mod state;

pub(crate) use render::ContextBudgetRenderResult;
pub(crate) use state::ContextBudgetState;

#[cfg(test)]
mod tests;
