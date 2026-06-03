mod cache;
mod item_index;
mod items;
mod list;
mod markdown;
pub(crate) mod markdown_highlight;
mod markdown_links;
mod markdown_render;
pub(crate) mod markdown_table_source;
mod prompt_wrap;
mod render_state;
mod wrap;

/// 主 transcript 中可被 Ctrl+T overlay 还原为完整内容的统一提示文案。
pub(crate) const TRANSCRIPT_DETAIL_HINT: &str = "ctrl + t to view transcript";

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
pub use items::ReasoningDisplayMode;
pub(crate) use items::ReasoningRenderMode;
pub(crate) use items::{
    FinalBodyDividerItem, ReasoningMessageItem, SystemMessageItem, WorkDurationMessageItem,
};
pub(crate) use list::{Transcript, TranscriptItem, materialize_transcript_item_render_block};
pub(crate) use markdown::{
    estimate_markdown_metrics_for_tabs, render_markdown_lines, render_markdown_metrics,
};
#[cfg(test)]
pub(crate) use markdown::{
    render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
};
pub(crate) use prompt_wrap::{PromptVisualLine, wrap_prompt_visual_lines};
#[cfg(test)]
pub(crate) use render_state::RenderItemSummary;
#[cfg(test)]
pub(crate) use render_state::ViewportRenderResult;
#[cfg(test)]
pub(crate) use render_state::new_render_result;
pub(crate) use render_state::{
    ItemLineAnchor, LineAnchor, LineAnchorKind, RenderResult, index_only_render_result,
    new_render_result_with_append_start,
};
pub(crate) use wrap::{
    DEFAULT_RENDER_WIDTH, display_tab_width, wrap_assistant_text, wrap_prompt_text,
};
pub(crate) use wrap::{WrapSegmentKind, should_start_new_wrap_segment, wrap_segment_kind};
#[cfg(test)]
pub(crate) use wrap::{prompt_text_wrap_call_count, reset_prompt_text_wrap_call_count};
