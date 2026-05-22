use std::path::Path;

#[cfg(test)]
use std::cell::Cell;

use pulldown_cmark::{Options, Parser};
use ratatui::text::Line;

use crate::theme::TerminalPalette;
use engine::MarkdownRenderer;
use wrapping::{count_leading_blank_lines, count_trailing_blank_lines};

mod engine;
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
) -> Vec<Line<'static>> {
    let cwd = std::env::current_dir().ok();
    render_markdown_lines_with_cwd(markdown, width, palette, cwd.as_deref())
}

fn render_markdown_lines_with_cwd(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
    cwd: Option<&Path>,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let leading_blank_lines = count_leading_blank_lines(markdown);
    let trailing_blank_lines = count_trailing_blank_lines(markdown);
    let mut renderer = MarkdownRenderer::new(palette, cwd, width);
    let options = markdown_options();

    renderer.render(Parser::new_ext(markdown, options));

    let mut lines = Vec::new();
    for _ in 0..leading_blank_lines {
        lines.push(Line::raw(""));
    }
    lines.extend(renderer.finish(width));
    for _ in 0..trailing_blank_lines {
        lines.push(Line::raw(""));
    }

    if lines.iter().all(|line| line.width() == 0) {
        return Vec::new();
    }

    lines
}

pub(crate) fn render_markdown_metrics(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
) -> (usize, usize) {
    #[cfg(test)]
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(|count| count.set(count.get() + 1));

    measure_markdown_metrics(markdown, width, palette)
}

pub(crate) fn estimate_markdown_metrics_for_tabs(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
) -> (usize, usize) {
    measure_markdown_metrics(markdown, width, palette)
}

fn measure_markdown_metrics(
    markdown: &str,
    width: usize,
    palette: TerminalPalette,
) -> (usize, usize) {
    let width = width.max(1);
    let leading_blank_lines = count_leading_blank_lines(markdown);
    let trailing_blank_lines = count_trailing_blank_lines(markdown);
    let cwd = std::env::current_dir().ok();
    let mut renderer = MarkdownRenderer::new_for_metrics(palette, cwd.as_deref(), width);
    let options = markdown_options();

    renderer.render(Parser::new_ext(markdown, options));

    let (line_count, plain_text_len) = renderer.finish_metrics(width);
    if plain_text_len == 0 {
        return (0, 0);
    }

    (
        line_count + leading_blank_lines + trailing_blank_lines,
        plain_text_len,
    )
}

fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);
    options
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
