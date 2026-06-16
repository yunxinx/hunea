//! Assistant Markdown rendering facade.
//!
//! 这里保留 transcript 层对 Markdown 渲染的公共入口；具体的
//! `pulldown-cmark` event 到 `ratatui` 行的转换放在 `markdown_render` 中。

pub(crate) use super::markdown_render::{
    assistant_markdown_options, estimate_markdown_metrics_for_tabs, render_markdown_lines,
    render_markdown_metrics,
};

#[cfg(test)]
pub(crate) use super::markdown_render::{
    render_markdown_metrics_call_count, reset_render_markdown_metrics_call_count,
};
