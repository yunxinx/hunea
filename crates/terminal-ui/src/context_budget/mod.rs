//! Context budget view (`/context`) overlay.

mod heatmap;
mod input;
mod layout;
mod legend;
mod render;
mod segment_colors;
mod state;

pub(crate) use state::ContextBudgetState;

#[cfg(test)]
mod tests;
