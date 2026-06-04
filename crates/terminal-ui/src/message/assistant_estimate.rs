use pulldown_cmark::{Event, Parser, Tag, TagEnd};
#[cfg(test)]
use std::cell::Cell;
use unicode_segmentation::UnicodeSegmentation;

use crate::transcript::{
    assistant_markdown_options,
    markdown_blocks::{
        MarkdownBlockKind, MarkdownLineBlockKind, MarkdownLineSeparator,
        markdown_line_spacing_before, markdown_list_line_kind,
    },
    wrap_assistant_text,
};
use crate::{display_width::grapheme_width, markdown_source::markdown_source_bounds};

#[cfg(test)]
thread_local! {
    static EXACT_SHAPE_CHECK_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

/// `assistant` Markdown metrics 快路径返回的渲染尺寸估计。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AssistantMarkdownMetrics {
    line_count: usize,
    char_len: usize,
}

impl AssistantMarkdownMetrics {
    /// 转换为调用方使用的 `(line_count, char_len)` metrics 形状。
    pub(super) const fn into_tuple(self) -> (usize, usize) {
        (self.line_count, self.char_len)
    }
}

/// 使用轻量逐行扫描估算常见 assistant Markdown 的渲染尺寸。
///
/// 该路径不运行 `pulldown-cmark` parser-backed exact 校验，只服务 resize/progressive
/// metrics 的近似估算；需要精确 renderer 语义时应调用
/// `estimate_common_markdown_metrics_exact_fast`。
pub(super) fn estimate_common_markdown_metrics_fast(
    content: &str,
    width: usize,
) -> Option<AssistantMarkdownMetrics> {
    estimate_common_markdown_metrics_fast_impl(content, width, true)
}

/// 使用保守 parser shape 校验估算可与 eager renderer 对齐的 Markdown 尺寸。
///
/// 只有 heading/list/fenced code 等简单结构能走该路径；遇到 inline Markdown、
/// entity、转义或 paragraph 时返回 `None`，交由 renderer-backed metrics 处理。
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
    let mut separator_before_line = MarkdownLineSeparator::Direct;
    let mut fence_marker = None;
    let mut saw_markdown_structure = false;
    let source_bounds = markdown_source_bounds(content);
    if source_bounds.is_empty() {
        return None;
    }

    for raw_line in content[source_bounds.content_start..source_bounds.content_end].lines() {
        if let Some(marker) = markdown_fence_marker(raw_line) {
            saw_markdown_structure = true;
            if fence_marker == Some(marker) {
                fence_marker = None;
                previous_block = Some(MarkdownLineBlockKind::Code);
            } else if fence_marker.is_none() {
                line_count = line_count.saturating_add(markdown_line_spacing_before(
                    previous_block,
                    MarkdownLineBlockKind::Code,
                    separator_before_line,
                ));
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
            if previous_block.is_some() {
                separator_before_line = MarkdownLineSeparator::Blank;
            }
            continue;
        }
        // 缩进非空行可能是 CommonMark list continuation、nested list 或 indented code。
        // 普通 fast metrics 是轻量估算路径，遇到这类需要 parser block 语义的内容
        // 直接回退，避免继续扩张手写 CommonMark 识别。
        if raw_line != trimmed {
            return None;
        }

        let block = if is_markdown_heading(trimmed) {
            saw_markdown_structure = true;
            if !allow_inexact
                && !line_matches_exact_renderer_shape(raw_line, MarkdownBlockKind::Heading)
            {
                return None;
            }
            MarkdownLineBlockKind::Heading
        } else if raw_line == trimmed
            && let Some(list_kind) = markdown_list_line_kind(trimmed)
        {
            saw_markdown_structure = true;
            if !allow_inexact
                && !line_matches_exact_renderer_shape(raw_line, MarkdownBlockKind::List)
            {
                return None;
            }
            MarkdownLineBlockKind::List(list_kind)
        } else {
            if !allow_inexact {
                return None;
            }
            MarkdownLineBlockKind::Paragraph
        };

        line_count = line_count.saturating_add(markdown_line_spacing_before(
            previous_block,
            block,
            separator_before_line,
        ));
        add_wrapped_assistant_estimate_line(raw_line, width, &mut line_count, &mut char_len);
        previous_block = Some(block);
        separator_before_line = MarkdownLineSeparator::Direct;
    }

    if !saw_markdown_structure || fence_marker.is_some() {
        return None;
    }

    Some(AssistantMarkdownMetrics {
        line_count: line_count
            .saturating_add(source_bounds.outer_blank_line_count())
            .max(1),
        char_len,
    })
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
        let cluster_width = grapheme_width(grapheme);
        if current_width > 0 && current_width.saturating_add(cluster_width) > width {
            count = count.saturating_add(1);
            current_width = 0;
        }
        current_width = current_width.saturating_add(cluster_width);
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

fn line_matches_exact_renderer_shape(line: &str, block: MarkdownBlockKind) -> bool {
    #[cfg(test)]
    EXACT_SHAPE_CHECK_CALL_COUNT.with(|count| count.set(count.get() + 1));

    let Some(plain_text) = parser_single_plain_text_for_exact_fast(line, block) else {
        return false;
    };

    match block {
        MarkdownBlockKind::Heading => {
            let marker_len = line.chars().take_while(|ch| *ch == '#').count();
            line.get(marker_len + 1..)
                .is_some_and(|source_text| source_text == plain_text.as_ref())
        }
        MarkdownBlockKind::List => line
            .strip_prefix("- ")
            .is_some_and(|source_text| source_text == plain_text.as_ref()),
        MarkdownBlockKind::Paragraph | MarkdownBlockKind::Code => false,
    }
}

fn parser_single_plain_text_for_exact_fast<'a>(
    line: &'a str,
    block: MarkdownBlockKind,
) -> Option<pulldown_cmark::CowStr<'a>> {
    let mut plain_text = None;
    for event in Parser::new_ext(line, assistant_markdown_options()) {
        match event {
            Event::Start(tag) if is_exact_fast_container_start(block, &tag) => {}
            Event::End(tag) if is_exact_fast_container_end(block, tag) => {}
            Event::Text(text) if plain_text.is_none() => plain_text = Some(text),
            _ => return None,
        }
    }

    plain_text
}

fn is_exact_fast_container_start(block: MarkdownBlockKind, tag: &Tag<'_>) -> bool {
    match block {
        MarkdownBlockKind::Heading => matches!(tag, Tag::Heading { .. }),
        MarkdownBlockKind::List => matches!(tag, Tag::List(None) | Tag::Item | Tag::Paragraph),
        MarkdownBlockKind::Paragraph | MarkdownBlockKind::Code => false,
    }
}

fn is_exact_fast_container_end(block: MarkdownBlockKind, tag: TagEnd) -> bool {
    match block {
        MarkdownBlockKind::Heading => matches!(tag, TagEnd::Heading(_)),
        MarkdownBlockKind::List => {
            matches!(tag, TagEnd::List(_) | TagEnd::Item | TagEnd::Paragraph)
        }
        MarkdownBlockKind::Paragraph | MarkdownBlockKind::Code => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::assistant::{
        assistant_message_content_width, render_assistant_message_metrics,
    };
    use super::*;
    use crate::{theme::default_palette, transcript::render_markdown_metrics};

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
    }

    #[test]
    fn common_markdown_fast_metrics_do_not_run_parser_shape_checks() {
        let markdown = concat!(
            "## AT&amp;T\n\n",
            "- use `cargo test`\n",
            "- [docs](https://example.com)\n",
        );

        EXACT_SHAPE_CHECK_CALL_COUNT.with(|count| count.set(0));
        assert!(estimate_common_markdown_metrics_fast(markdown, 80).is_some());
        assert_eq!(
            EXACT_SHAPE_CHECK_CALL_COUNT.with(Cell::get),
            0,
            "ordinary fast metrics should stay on the lightweight line scanner"
        );
    }

    #[test]
    fn assistant_metrics_match_renderer_for_heading_followed_by_list() {
        let markdown = concat!(
            "### 1. 构词逻辑\n",
            "*   **Q**：取自英文单词 **Question**（问题）。\n\n",
            "### 2. 品牌寓意\n",
            "将二者结合。\n",
        );
        let width = 80;
        let palette = default_palette();

        assert_eq!(
            estimate_common_markdown_metrics_exact_fast(markdown, width),
            None,
            "inline Markdown inside heading/list text should use the renderer-backed exact path"
        );
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
    fn common_markdown_fast_exact_metrics_reject_parser_inline_events() {
        for markdown in [
            "## AT&amp;T\n\n- plain item",
            "## escaped \\* marker\n\n- plain item",
            "## plain\n\n- use `cargo test`",
            "## plain\n\n- [docs](https://example.com)",
        ] {
            assert_eq!(
                estimate_common_markdown_metrics_exact_fast(markdown, 80),
                None,
                "parser-visible inline Markdown should use the renderer-backed exact path: {markdown}"
            );
        }
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
    fn common_markdown_fast_metrics_preserve_blank_line_between_paragraph_blocks() {
        let markdown = "## Section\n\nfirst paragraph\n\nsecond paragraph";
        let width = 80;
        let palette = default_palette();

        assert_eq!(
            estimate_common_markdown_metrics_fast(markdown, width)
                .map(AssistantMarkdownMetrics::into_tuple),
            Some(render_markdown_metrics(markdown, width, palette))
        );
    }

    #[test]
    fn common_markdown_fast_metrics_match_renderer_for_blank_separated_list_items() {
        let markdown = "## Section\n\n- first item\n\n- second item";
        let width = 80;
        let palette = default_palette();

        assert_eq!(
            estimate_common_markdown_metrics_fast(markdown, width)
                .map(AssistantMarkdownMetrics::into_tuple),
            Some(render_markdown_metrics(markdown, width, palette))
        );
    }

    #[test]
    fn common_markdown_fast_metrics_fall_back_for_list_continuation_lines() {
        let markdown = concat!(
            "## Section\n\n",
            "- first item\n",
            "  continuation belongs to the first list item\n\n",
            "- second item\n",
        );

        assert_eq!(
            estimate_common_markdown_metrics_fast(markdown, 80),
            None,
            "普通 fast metrics 不复制 CommonMark list continuation 语义"
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
