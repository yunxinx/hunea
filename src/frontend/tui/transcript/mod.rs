mod cache;
mod list;
mod markdown;
mod prompt_wrap;
mod render_state;
mod wrap;

pub(crate) use cache::CachedLineAnchors;
#[cfg(test)]
pub(crate) use cache::CachedRenderBlock;
pub(crate) use list::{Transcript, TranscriptItem};
pub(crate) use markdown::render_markdown_lines;
pub(crate) use prompt_wrap::{PromptVisualLine, wrap_prompt_visual_lines};
#[cfg(test)]
pub(crate) use render_state::RenderItemSummary;
#[cfg(test)]
pub(crate) use render_state::new_render_result;
pub(crate) use render_state::{
    ItemLineAnchor, LineAnchor, LineAnchorKind, RenderResult, ViewportRenderResult,
    new_render_result_with_append_start,
};
pub(crate) use wrap::{DEFAULT_RENDER_WIDTH, wrap_assistant_text, wrap_prompt_text};
