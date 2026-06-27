//! Context budget view (`/context`) overlay.

mod heatmap;
mod input;
mod render;
mod segment_colors;
mod state;

pub(crate) use state::ContextBudgetState;

#[cfg(test)]
mod tests;
