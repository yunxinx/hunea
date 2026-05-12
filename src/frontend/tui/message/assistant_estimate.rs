use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::transcript::wrap_assistant_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantMarkdownBlock {
    Heading,
    List,
    Paragraph,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AssistantMarkdownMetrics {
    line_count: usize,
    char_len: usize,
    is_exact: bool,
}

impl AssistantMarkdownMetrics {
    pub(super) const fn into_tuple(self) -> (usize, usize) {
        (self.line_count, self.char_len)
    }
}

pub(super) fn estimate_common_markdown_metrics_fast(
    content: &str,
    width: usize,
) -> Option<AssistantMarkdownMetrics> {
    estimate_common_markdown_metrics_fast_impl(content, width, true)
}

pub(super) fn estimate_common_markdown_metrics_exact_fast(
    content: &str,
    width: usize,
) -> Option<AssistantMarkdownMetrics> {
    estimate_common_markdown_metrics_fast_impl(content, width, false)
}

fn estimate_common_markdown_metrics_fast_impl(
    content: &str,
    width: usize,
    allow_inexact: bool,
) -> Option<AssistantMarkdownMetrics> {
    let mut line_count = 0usize;
    let mut char_len = 0usize;
    let mut previous_block = None;
    let mut fence_marker = None;
    let mut saw_markdown_structure = false;
    let mut saw_paragraph = false;
    let outer_blank_lines = count_markdown_outer_blank_lines(content);
    let lines = content.lines().collect::<Vec<_>>();
    let last_content_line = lines.iter().rposition(|line| !line.trim().is_empty())?;

    for raw_line in &lines[..=last_content_line] {
        if let Some(marker) = markdown_fence_marker(raw_line) {
            saw_markdown_structure = true;
            if fence_marker == Some(marker) {
                fence_marker = None;
                previous_block = Some(AssistantMarkdownBlock::Code);
            } else if fence_marker.is_none() {
                if should_insert_markdown_spacing(previous_block, AssistantMarkdownBlock::Code) {
                    line_count = line_count.saturating_add(1);
                }
                fence_marker = Some(marker);
            }
            continue;
        }

        if fence_marker.is_some() {
            add_literal_assistant_estimate_line(raw_line, width, &mut line_count, &mut char_len);
            continue;
        }

        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() {
            continue;
        }

        let block = if is_markdown_heading(trimmed) {
            saw_markdown_structure = true;
            AssistantMarkdownBlock::Heading
        } else if *raw_line == trimmed && is_markdown_list_item(trimmed) {
            saw_markdown_structure = true;
            AssistantMarkdownBlock::List
        } else {
            if !allow_inexact {
                return None;
            }
            saw_paragraph = true;
            AssistantMarkdownBlock::Paragraph
        };

        if should_insert_markdown_spacing(previous_block, block) {
            line_count = line_count.saturating_add(1);
        }
        add_wrapped_assistant_estimate_line(raw_line, width, &mut line_count, &mut char_len);
        previous_block = Some(block);
    }

    if !saw_markdown_structure || fence_marker.is_some() {
        return None;
    }

    Some(AssistantMarkdownMetrics {
        line_count: line_count.saturating_add(outer_blank_lines).max(1),
        char_len,
        is_exact: !saw_paragraph,
    })
}

fn count_markdown_outer_blank_lines(content: &str) -> usize {
    content
        .split('\n')
        .take_while(|line| line.is_empty())
        .count()
        + content
            .rsplit('\n')
            .take_while(|line| line.is_empty())
            .count()
}

fn should_insert_markdown_spacing(
    previous_block: Option<AssistantMarkdownBlock>,
    next_block: AssistantMarkdownBlock,
) -> bool {
    let Some(previous_block) = previous_block else {
        return false;
    };
    if previous_block == AssistantMarkdownBlock::List || next_block == AssistantMarkdownBlock::List
    {
        return false;
    }
    true
}

fn add_wrapped_assistant_estimate_line(
    line: &str,
    width: usize,
    line_count: &mut usize,
    char_len: &mut usize,
) {
    let wrapped = wrap_assistant_text(line, width, 0);
    if wrapped.is_empty() {
        *line_count = (*line_count).saturating_add(1);
        return;
    }

    *line_count = (*line_count).saturating_add(wrapped.len());
    *char_len = (*char_len).saturating_add(wrapped.iter().map(String::len).sum::<usize>());
}

fn add_literal_assistant_estimate_line(
    line: &str,
    width: usize,
    line_count: &mut usize,
    char_len: &mut usize,
) {
    *line_count = (*line_count).saturating_add(hard_wrapped_line_count(line, width));
    *char_len = (*char_len).saturating_add(line.len());
}

fn hard_wrapped_line_count(line: &str, width: usize) -> usize {
    if line.is_empty() {
        return 1;
    }

    let width = width.max(1);
    let mut count = 1usize;
    let mut current_width = 0usize;
    for grapheme in UnicodeSegmentation::graphemes(line, true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if current_width > 0 && current_width.saturating_add(grapheme_width) > width {
            count = count.saturating_add(1);
            current_width = 0;
        }
        current_width = current_width.saturating_add(grapheme_width);
    }

    count
}

fn markdown_fence_marker(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") {
        return Some("```");
    }
    if trimmed.starts_with("~~~") {
        return Some("~~~");
    }
    None
}

fn is_markdown_heading(trimmed_line: &str) -> bool {
    let marker_len = trimmed_line.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&marker_len)
        && trimmed_line
            .chars()
            .nth(marker_len)
            .is_some_and(char::is_whitespace)
}

fn is_markdown_list_item(trimmed_line: &str) -> bool {
    let mut chars = trimmed_line.chars();
    matches!(chars.next(), Some('-' | '*' | '+')) && chars.next().is_some_and(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::super::assistant::{
        assistant_message_content_width, render_assistant_message_metrics,
    };
    use super::*;
    use crate::frontend::tui::{theme::default_palette, transcript::render_markdown_metrics};

    #[test]
    fn common_markdown_fast_metrics_match_renderer_for_fenced_lists() {
        let markdown = concat!(
            "## Assistant 01\n\n",
            "- summarize viewport recovery\n",
            "- explain transcript cache reuse\n",
            "- keep document layout stable\n\n",
            "```rust\n",
            "fn assistant_1() -> &'static str {\n",
            "    \"benchmark content benchmark content benchmark content benchmark content ",
            "benchmark content benchmark content \"\n",
            "}\n",
            "```\n",
        );
        let width = 76;

        assert_eq!(
            estimate_common_markdown_metrics_fast(markdown, width)
                .map(|metrics| metrics.into_tuple()),
            Some(render_markdown_metrics(markdown, width, default_palette()))
        );
        assert!(
            estimate_common_markdown_metrics_fast(markdown, width)
                .is_some_and(|metrics| metrics.is_exact)
        );
    }

    #[test]
    fn assistant_metrics_fall_back_to_renderer_for_paragraph_markdown() {
        let markdown = concat!(
            "## Section\n\n",
            "- list item\n\n",
            "```rust\n",
            "fn section() {}\n",
            "```\n\n",
            "Follow-up prose wraps through the document viewport without forcing full-frame work.",
        );
        let width = 40;
        let palette = default_palette();

        assert_eq!(
            render_assistant_message_metrics(markdown, width as u16, palette),
            render_markdown_metrics(markdown, width, palette)
        );
    }

    #[test]
    fn common_markdown_fast_exact_metrics_preserve_outer_blank_lines() {
        let markdown = concat!("\n", "## Section\n\n", "- list item\n\n");
        let width = 40;
        let palette = default_palette();

        assert_eq!(
            render_assistant_message_metrics(markdown, width as u16, palette),
            render_markdown_metrics(
                markdown,
                assistant_message_content_width(width as u16),
                palette
            )
        );
    }

    #[test]
    fn assistant_metrics_fall_back_to_renderer_for_nested_lists() {
        let markdown = "- outer\n  - inner";
        let width = 40;
        let palette = default_palette();

        assert_eq!(
            render_assistant_message_metrics(markdown, width as u16, palette),
            render_markdown_metrics(
                markdown,
                assistant_message_content_width(width as u16),
                palette
            )
        );
    }
}
