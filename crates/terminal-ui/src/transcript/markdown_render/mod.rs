//! Assistant Markdown 的宽度敏感渲染与指标估算。

use std::path::Path;

#[cfg(test)]
use std::cell::Cell;

use pulldown_cmark::{Options, Parser};
use ratatui::text::Line;

use crate::{
    display_width::line_display_width, markdown_source::markdown_source_bounds,
    terminal_text::sanitize_terminal_text, theme::TerminalPalette,
    transcript::markdown_table_source::unwrap_markdown_table_fences,
};
use engine::MarkdownRenderer;

mod engine;
mod table;
mod wrapping;

#[cfg(test)]
thread_local! {
    static RENDER_MARKDOWN_METRICS_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// `render_markdown_lines` 把 assistant Markdown 渲染成宽度敏感的最终文本行。
pub(crate) fn render_markdown_lines(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> Vec<Line<'static>> {
    render_markdown_lines_with_cwd(
        markdown,
        width,
        palette,
        working_dir,
        MarkdownProfile::Assistant,
    )
}

fn render_markdown_lines_with_cwd(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    cwd: Option<&Path>,
    profile: MarkdownProfile,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let sanitized_markdown = sanitize_terminal_text(markdown);
    let normalized_markdown = profile
        .unwraps_markdown_table_fences()
        .then(|| unwrap_markdown_table_fences(sanitized_markdown.as_ref()));
    let markdown = normalized_markdown
        .as_ref()
        .map(std::borrow::Cow::as_ref)
        .unwrap_or_else(|| sanitized_markdown.as_ref());
    let source_bounds = markdown_source_bounds(markdown);
    let mut renderer = profile.renderer(palette, cwd, width);
    let options = profile.options();

    renderer.render(
        markdown,
        Parser::new_ext(markdown, options).into_offset_iter(),
    );

    let mut lines = Vec::new();
    for _ in 0..source_bounds.leading_blank_lines {
        lines.push(Line::raw(""));
    }
    lines.extend(renderer.finish(width));
    for _ in 0..source_bounds.trailing_blank_lines {
        lines.push(Line::raw(""));
    }

    if lines.iter().all(|line| line_display_width(line) == 0) {
        return Vec::new();
    }

    lines
}

/// `render_reasoning_markdown_lines` 使用 codex-rs reasoning summary 的 Markdown profile。
///
/// 这里不复用 assistant profile：Reasoning Content 是现有 reasoning 样式的增强，
/// 只启用 `tables + strikethrough`，不继承 assistant 的 task list/math 语义，也不解包
/// markdown fence 中的表格。
pub(crate) fn render_reasoning_markdown_lines(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> Vec<Line<'static>> {
    render_markdown_lines_with_cwd(
        markdown,
        width,
        palette,
        working_dir,
        MarkdownProfile::Reasoning,
    )
}

pub(crate) fn render_markdown_metrics(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> (usize, usize) {
    #[cfg(test)]
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(|count| count.set(count.get() + 1));

    measure_markdown_metrics(markdown, width, palette, working_dir)
}

pub(crate) fn render_reasoning_markdown_metrics(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> (usize, usize) {
    measure_markdown_metrics_with_profile(
        markdown,
        width,
        palette,
        working_dir,
        MarkdownProfile::Reasoning,
    )
}

pub(crate) fn estimate_markdown_metrics_for_tabs(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> (usize, usize) {
    measure_markdown_metrics(markdown, width, palette, working_dir)
}

fn measure_markdown_metrics(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
) -> (usize, usize) {
    measure_markdown_metrics_with_profile(
        markdown,
        width,
        palette,
        working_dir,
        MarkdownProfile::Assistant,
    )
}

fn measure_markdown_metrics_with_profile(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    working_dir: Option<&Path>,
    profile: MarkdownProfile,
) -> (usize, usize) {
    let width = width.max(1);
    let normalized_markdown = profile
        .unwraps_markdown_table_fences()
        .then(|| unwrap_markdown_table_fences(markdown));
    let markdown = normalized_markdown
        .as_ref()
        .map(std::borrow::Cow::as_ref)
        .unwrap_or(markdown);
    let source_bounds = markdown_source_bounds(markdown);
    let mut renderer = profile.metrics_renderer(palette, working_dir, width);
    let options = profile.options();

    renderer.render(
        markdown,
        Parser::new_ext(markdown, options).into_offset_iter(),
    );

    let (line_count, plain_text_len) = renderer.finish_metrics(width);
    if plain_text_len == 0 {
        return (0, 0);
    }

    (
        line_count + source_bounds.outer_blank_line_count(),
        plain_text_len,
    )
}

/// 返回 assistant Markdown renderer 使用的 pulldown-cmark options。
pub(crate) fn assistant_markdown_options() -> Options {
    MarkdownProfile::Assistant.options()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownProfile {
    Assistant,
    Reasoning,
}

impl MarkdownProfile {
    fn options(self) -> Options {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        options.insert(Options::ENABLE_STRIKETHROUGH);
        if matches!(self, Self::Assistant) {
            options.insert(Options::ENABLE_TASKLISTS);
            options.insert(Options::ENABLE_MATH);
        }
        options
    }

    fn unwraps_markdown_table_fences(self) -> bool {
        matches!(self, Self::Assistant)
    }

    fn renderer<'a>(
        self,
        palette: TerminalPalette,
        cwd: Option<&'a Path>,
        width: usize,
    ) -> MarkdownRenderer<'a> {
        match self {
            Self::Assistant => MarkdownRenderer::new(palette, cwd, width),
            Self::Reasoning => MarkdownRenderer::new_reasoning(palette, cwd, width),
        }
    }

    fn metrics_renderer<'a>(
        self,
        palette: TerminalPalette,
        cwd: Option<&'a Path>,
        width: usize,
    ) -> MarkdownRenderer<'a> {
        match self {
            Self::Assistant => MarkdownRenderer::new_for_metrics(palette, cwd, width),
            Self::Reasoning => MarkdownRenderer::new_reasoning_for_metrics(palette, cwd, width),
        }
    }
}

#[cfg(test)]
pub(crate) fn reset_render_markdown_metrics_call_count() {
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn render_markdown_metrics_call_count() -> usize {
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(Cell::get)
}

#[cfg(test)]
mod tests;
