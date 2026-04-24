mod cache;
mod item_index;
mod list;
mod markdown;
mod prompt_wrap;
mod render_state;
mod wrap;

#[cfg(test)]
pub(crate) use cache::CachedLineAnchors;
pub(crate) use cache::viewport_overscan_line_budget;
pub(crate) use cache::{CachedRenderBlock, RetainedBlockMemorySummary};
#[cfg(test)]
pub(crate) use cache::{
    reset_tracked_cached_render_block_access, tracked_cached_render_block_access,
};
pub(crate) use item_index::{
    TranscriptEstimateBreakdown, TranscriptEstimateKind, TranscriptEstimateSource,
    TranscriptFastEstimate, TranscriptItemMetrics, TranscriptItemMetricsCache,
    TranscriptItemMetricsIndex, TranscriptItemMetricsQuality, TranscriptItemPosition,
};
pub(crate) use list::{Transcript, TranscriptItem, materialize_transcript_item_render_block};
pub(crate) use markdown::{render_markdown_lines, render_markdown_metrics};
#[cfg(test)]
pub(crate) use markdown::{
    render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
};
pub(crate) use prompt_wrap::{PromptVisualLine, wrap_prompt_visual_lines};
#[cfg(test)]
pub(crate) use render_state::RenderItemSummary;
#[cfg(test)]
pub(crate) use render_state::new_render_result;
pub(crate) use render_state::{
    ItemLineAnchor, LineAnchor, LineAnchorKind, RenderResult, ViewportRenderResult,
    index_only_render_result, new_render_result_with_append_start,
};
pub(crate) use wrap::{
    DEFAULT_RENDER_WIDTH, display_tab_width, wrap_assistant_text, wrap_prompt_text,
};
