use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    styled_text::{line_plain_text_len, line_to_plain_text},
    theme::TerminalPalette,
    transcript::{
        markdown_table_source::contains_table_structure, render_markdown_lines,
        render_markdown_metrics,
    },
};

use super::assistant::assistant_message_content_width;

const ASSISTANT_PROJECTION_MIN_BYTES: usize = 4 * 1024;
const ASSISTANT_PROJECTION_PAGE_LINES: usize = 64;

type AssistantProjectionPage = Rc<Vec<Line<'static>>>;
type AssistantProjectionPageKey = (usize, usize);
type AssistantProjectionPageCache =
    RefCell<HashMap<AssistantProjectionPageKey, AssistantProjectionPage>>;

/// `AssistantMessageRenderProjection` 保存长 assistant Markdown 的按需渲染视图。
#[derive(Debug)]
pub(crate) struct AssistantMessageRenderProjection {
    source: Rc<str>,
    width: usize,
    palette: TerminalPalette,
    blocks: Vec<AssistantProjectedBlock>,
    pages: AssistantProjectionPageCache,
    line_count: usize,
    plain_text_char_len: usize,
}

#[derive(Debug)]
struct AssistantProjectedBlock {
    start_line: usize,
    line_count: usize,
    leading_blank_lines: usize,
    char_len: usize,
    kind: AssistantProjectedBlockKind,
}

#[derive(Debug)]
enum AssistantProjectedBlockKind {
    Blank,
    MarkdownSnippet {
        range: Range<usize>,
    },
    FencedCode {
        marker: &'static str,
        info_range: Range<usize>,
        line_ranges: Vec<Range<usize>>,
        line_prefix_sums: Vec<usize>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssistantMarkdownBlock {
    Heading,
    List,
    Paragraph,
    Code,
}

#[derive(Debug, Clone, Copy)]
struct AssistantSourceLine<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

struct FencedCodePageRender<'a> {
    source: &'a str,
    marker: &'static str,
    info_range: &'a Range<usize>,
    line_ranges: &'a [Range<usize>],
    line_prefix_sums: &'a [usize],
    page_start: usize,
    page_end: usize,
    width: usize,
    palette: TerminalPalette,
}

impl AssistantMessageRenderProjection {
    pub(crate) fn line_count(&self) -> usize {
        self.line_count
    }

    pub(crate) fn plain_text_char_len(&self) -> usize {
        self.plain_text_char_len
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<Line<'static>> {
        let (block_index, block) = self.block_for_line(index)?;
        block.line_at(
            &self.source,
            &self.pages,
            block_index,
            self.width,
            self.palette,
            index - block.start_line,
        )
    }

    pub(crate) fn plain_line_at(&self, index: usize) -> Option<String> {
        self.line_at(index).map(|line| line_to_plain_text(&line))
    }

    pub(crate) fn plain_line_len(&self, index: usize) -> Option<usize> {
        self.line_at(index).map(|line| line_plain_text_len(&line))
    }

    pub(crate) fn estimated_render_ui_bytes(&self) -> usize {
        let pages = self.pages.borrow();
        let cached_page_bytes = pages
            .values()
            .map(|page| estimated_page_bytes(page.as_slice()))
            .sum::<usize>();
        let page_map_bytes = pages.capacity()
            * (std::mem::size_of::<AssistantProjectionPageKey>()
                + std::mem::size_of::<AssistantProjectionPage>());

        std::mem::size_of::<Self>()
            + std::mem::size_of_val(&self.pages)
            + std::mem::size_of_val(self.blocks.as_slice())
            + self
                .blocks
                .iter()
                .map(AssistantProjectedBlock::estimated_index_bytes)
                .sum::<usize>()
            + page_map_bytes
            + cached_page_bytes
    }

    fn block_for_line(&self, index: usize) -> Option<(usize, &AssistantProjectedBlock)> {
        let block_index = self
            .blocks
            .partition_point(|block| block.start_line <= index)
            .checked_sub(1)?;
        let block = self.blocks.get(block_index)?;
        (index < block.start_line + block.line_count).then_some((block_index, block))
    }
}

impl AssistantProjectedBlock {
    fn blank(start_line: usize) -> Self {
        Self {
            start_line,
            line_count: 1,
            leading_blank_lines: 0,
            char_len: 0,
            kind: AssistantProjectedBlockKind::Blank,
        }
    }

    fn markdown_snippet(
        start_line: usize,
        leading_blank_lines: usize,
        source: &str,
        range: Range<usize>,
        width: usize,
        palette: TerminalPalette,
    ) -> Option<Self> {
        let snippet = &source[range.clone()];
        let (line_count, char_len) = render_markdown_metrics(snippet, width, palette);
        (line_count > 0).then(|| Self {
            start_line,
            line_count: line_count.saturating_add(leading_blank_lines),
            leading_blank_lines,
            char_len,
            kind: AssistantProjectedBlockKind::MarkdownSnippet { range },
        })
    }

    fn fenced_code(
        start_line: usize,
        leading_blank_lines: usize,
        marker: &'static str,
        source: &str,
        info_range: Range<usize>,
        line_ranges: Vec<Range<usize>>,
        width: usize,
    ) -> Self {
        let mut line_prefix_sums = Vec::with_capacity(line_ranges.len() + 1);
        line_prefix_sums.push(0);
        let mut char_len = 0usize;
        for range in &line_ranges {
            let line = &source[range.clone()];
            let line_count = hard_wrapped_line_count(line, width);
            line_prefix_sums.push(
                line_prefix_sums
                    .last()
                    .copied()
                    .unwrap_or(0usize)
                    .saturating_add(line_count),
            );
            char_len = char_len.saturating_add(line.len());
        }

        let line_count = line_prefix_sums
            .last()
            .copied()
            .unwrap_or(0)
            .max(1)
            .saturating_add(leading_blank_lines);
        Self {
            start_line,
            line_count,
            leading_blank_lines,
            char_len,
            kind: AssistantProjectedBlockKind::FencedCode {
                marker,
                info_range,
                line_ranges,
                line_prefix_sums,
            },
        }
    }

    fn line_at(
        &self,
        source: &str,
        pages: &AssistantProjectionPageCache,
        block_index: usize,
        width: usize,
        palette: TerminalPalette,
        relative_line: usize,
    ) -> Option<Line<'static>> {
        if relative_line >= self.line_count {
            return None;
        }

        let page_index = relative_line / ASSISTANT_PROJECTION_PAGE_LINES;
        let page_offset = relative_line % ASSISTANT_PROJECTION_PAGE_LINES;
        let page = self.materialized_page(source, pages, block_index, page_index, width, palette);
        page.get(page_offset).cloned()
    }

    fn materialized_page(
        &self,
        source: &str,
        pages: &AssistantProjectionPageCache,
        block_index: usize,
        page_index: usize,
        width: usize,
        palette: TerminalPalette,
    ) -> AssistantProjectionPage {
        let cache_key = (block_index, page_index);
        if let Some(page) = pages.borrow().get(&cache_key) {
            return Rc::clone(page);
        }

        let page_start = page_index * ASSISTANT_PROJECTION_PAGE_LINES;
        let page_end = (page_start + ASSISTANT_PROJECTION_PAGE_LINES).min(self.line_count);
        let blank_end = self.leading_blank_lines.min(page_end);
        let mut lines = Vec::new();
        if page_start < blank_end {
            lines.extend((page_start..blank_end).map(|_| Line::raw("")));
        }

        let content_start = page_start.saturating_sub(self.leading_blank_lines);
        let content_end = page_end.saturating_sub(self.leading_blank_lines);
        if content_start < content_end {
            lines.extend(match &self.kind {
                AssistantProjectedBlockKind::Blank => (content_start..content_end)
                    .map(|_| Line::raw(""))
                    .collect(),
                AssistantProjectedBlockKind::MarkdownSnippet { range } => {
                    let snippet = &source[range.clone()];
                    let lines = render_markdown_lines(snippet, width, palette);
                    let end = content_end.min(lines.len());
                    if content_start >= end {
                        Vec::new()
                    } else {
                        lines[content_start..end].to_vec()
                    }
                }
                AssistantProjectedBlockKind::FencedCode {
                    marker,
                    info_range,
                    line_ranges,
                    line_prefix_sums,
                } => render_fenced_code_page(FencedCodePageRender {
                    source,
                    marker,
                    info_range,
                    line_ranges,
                    line_prefix_sums,
                    page_start: content_start,
                    page_end: content_end,
                    width,
                    palette,
                }),
            });
        }

        let page = Rc::new(lines);
        pages.borrow_mut().insert(cache_key, Rc::clone(&page));
        page
    }

    fn estimated_index_bytes(&self) -> usize {
        match &self.kind {
            AssistantProjectedBlockKind::Blank
            | AssistantProjectedBlockKind::MarkdownSnippet { .. } => 0,
            AssistantProjectedBlockKind::FencedCode {
                line_ranges,
                line_prefix_sums,
                ..
            } => {
                std::mem::size_of_val(line_ranges.as_slice())
                    + std::mem::size_of_val(line_prefix_sums.as_slice())
            }
        }
    }
}

pub(super) fn render_assistant_message_projection(
    content: Rc<str>,
    width: u16,
    palette: TerminalPalette,
) -> Option<AssistantMessageRenderProjection> {
    if content.len() < ASSISTANT_PROJECTION_MIN_BYTES || content.contains('\t') {
        return None;
    }

    let width = assistant_message_content_width(width);
    let blocks = build_common_markdown_projection_blocks(content.as_ref(), width, palette)?;
    let line_count = blocks
        .last()
        .map(|block| block.start_line + block.line_count)
        .unwrap_or(0);
    let plain_text_char_len = blocks.iter().map(|block| block.char_len).sum();

    Some(AssistantMessageRenderProjection {
        source: content,
        width,
        palette,
        blocks,
        pages: RefCell::new(HashMap::new()),
        line_count,
        plain_text_char_len,
    })
}

fn build_common_markdown_projection_blocks(
    content: &str,
    width: usize,
    palette: TerminalPalette,
) -> Option<Vec<AssistantProjectedBlock>> {
    if contains_table_structure(content) {
        return None;
    }

    let split_lines = content.split('\n').collect::<Vec<_>>();
    let leading_blank_lines = split_lines
        .iter()
        .take_while(|line| line.is_empty())
        .count();
    let trailing_blank_lines = split_lines
        .iter()
        .rev()
        .take_while(|line| line.is_empty())
        .count();
    let source_lines = collect_source_lines(content);
    let last_content_line = source_lines
        .iter()
        .rposition(|line| !line.text.trim().is_empty())?;

    let mut blocks = Vec::new();
    let mut line_cursor = 0usize;
    for _ in 0..leading_blank_lines {
        push_block(&mut blocks, &mut line_cursor, |start_line| {
            AssistantProjectedBlock::blank(start_line)
        });
    }

    let mut saw_markdown_structure = false;
    let mut previous_block = None;
    let mut index = leading_blank_lines.min(last_content_line.saturating_add(1));
    while index <= last_content_line {
        let source_line = source_lines[index];
        let raw_line = source_line.text;
        let trimmed = raw_line.trim_start();
        if trimmed.is_empty() {
            index += 1;
            continue;
        }

        if let Some((marker, info)) = markdown_fence_opener(raw_line) {
            saw_markdown_structure = true;
            let leading_blank_lines =
                markdown_spacing_before(previous_block, AssistantMarkdownBlock::Code);

            let info_range = markdown_fence_info_range(source_line, marker)?;
            let mut code_lines = Vec::new();
            index += 1;
            let mut found_closing_fence = false;
            while index <= last_content_line {
                let candidate = source_lines[index];
                if is_markdown_fence_closer(candidate.text, marker) {
                    found_closing_fence = true;
                    break;
                }
                code_lines.push(candidate);
                index += 1;
            }
            if !found_closing_fence {
                return None;
            }
            if code_lines.iter().all(|line| line.text.is_empty()) {
                return None;
            }
            if fenced_code_requires_full_context(info, &code_lines) {
                return None;
            }
            let line_ranges = code_lines
                .iter()
                .map(|line| line.start..line.end)
                .collect::<Vec<_>>();
            push_block(&mut blocks, &mut line_cursor, |start_line| {
                AssistantProjectedBlock::fenced_code(
                    start_line,
                    leading_blank_lines,
                    marker,
                    content,
                    info_range,
                    line_ranges,
                    width,
                )
            });
            previous_block = Some(AssistantMarkdownBlock::Code);
            index += 1;
            continue;
        }

        if is_projectable_markdown_heading(raw_line, trimmed) {
            saw_markdown_structure = true;
            let leading_blank_lines =
                markdown_spacing_before(previous_block, AssistantMarkdownBlock::Heading);
            let block = AssistantProjectedBlock::markdown_snippet(
                line_cursor,
                leading_blank_lines,
                content,
                source_line.start..source_line.end,
                width,
                palette,
            )?;
            push_optional_block(&mut blocks, &mut line_cursor, block);
            previous_block = Some(AssistantMarkdownBlock::Heading);
            index += 1;
            continue;
        }

        if raw_line == trimmed && is_markdown_list_item(trimmed) {
            saw_markdown_structure = true;
            let start = index;
            index += 1;
            while index <= last_content_line {
                let candidate = source_lines[index].text;
                let candidate_trimmed = candidate.trim_start();
                if candidate_trimmed.is_empty() {
                    break;
                }
                if candidate != candidate_trimmed || !is_markdown_list_item(candidate_trimmed) {
                    break;
                }
                index += 1;
            }
            let leading_blank_lines =
                markdown_spacing_before(previous_block, AssistantMarkdownBlock::List);
            let block = AssistantProjectedBlock::markdown_snippet(
                line_cursor,
                leading_blank_lines,
                content,
                source_line_range(&source_lines, start, index)?,
                width,
                palette,
            )?;
            push_optional_block(&mut blocks, &mut line_cursor, block);
            previous_block = Some(AssistantMarkdownBlock::List);
            continue;
        }

        if raw_line != trimmed && is_markdown_list_item(trimmed) {
            return None;
        }

        if starts_with_unsupported_fence(raw_line) {
            return None;
        }

        if paragraph_line_is_projectable(raw_line) {
            let start = index;
            index += 1;
            while index <= last_content_line {
                let candidate = source_lines[index].text;
                let candidate_trimmed = candidate.trim_start();
                if candidate_trimmed.is_empty()
                    || markdown_fence_opener(candidate).is_some()
                    || starts_with_unsupported_fence(candidate)
                    || is_projectable_markdown_heading(candidate, candidate_trimmed)
                    || (candidate == candidate_trimmed && is_markdown_list_item(candidate_trimmed))
                {
                    break;
                }
                if candidate != candidate_trimmed && is_markdown_list_item(candidate_trimmed) {
                    return None;
                }
                if !paragraph_line_is_projectable(candidate) {
                    return None;
                }
                index += 1;
            }

            let leading_blank_lines =
                markdown_spacing_before(previous_block, AssistantMarkdownBlock::Paragraph);
            let block = AssistantProjectedBlock::markdown_snippet(
                line_cursor,
                leading_blank_lines,
                content,
                source_line_range(&source_lines, start, index)?,
                width,
                palette,
            )?;
            push_optional_block(&mut blocks, &mut line_cursor, block);
            previous_block = Some(AssistantMarkdownBlock::Paragraph);
            continue;
        }

        return None;
    }

    if !saw_markdown_structure {
        return None;
    }

    for _ in 0..trailing_blank_lines {
        push_block(&mut blocks, &mut line_cursor, |start_line| {
            AssistantProjectedBlock::blank(start_line)
        });
    }

    Some(blocks)
}

fn collect_source_lines(content: &str) -> Vec<AssistantSourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for segment in content.split_inclusive('\n') {
        let segment_end = start + segment.len();
        let mut text_end = segment_end;
        if segment.ends_with('\n') {
            text_end = text_end.saturating_sub(1);
        }
        if text_end > start && content[start..text_end].ends_with('\r') {
            text_end = text_end.saturating_sub(1);
        }
        lines.push(AssistantSourceLine {
            text: &content[start..text_end],
            start,
            end: text_end,
        });
        start = segment_end;
    }

    if start < content.len() {
        lines.push(AssistantSourceLine {
            text: &content[start..],
            start,
            end: content.len(),
        });
    }

    lines
}

fn source_line_range(
    source_lines: &[AssistantSourceLine<'_>],
    start: usize,
    end: usize,
) -> Option<Range<usize>> {
    let first = source_lines.get(start)?;
    let last = source_lines.get(end.checked_sub(1)?)?;
    Some(first.start..last.end)
}

fn markdown_fence_info_range(
    line: AssistantSourceLine<'_>,
    marker: &'static str,
) -> Option<Range<usize>> {
    let trimmed = line.text.trim_start();
    let indent_len = line.text.len().saturating_sub(trimmed.len());
    let info_start = line.start + indent_len + marker.len();
    (info_start <= line.end).then_some(info_start..line.end)
}

fn fenced_code_requires_full_context(info: &str, lines: &[AssistantSourceLine<'_>]) -> bool {
    let language = info
        .split([' ', '\t', ','])
        .next()
        .map(str::trim)
        .unwrap_or_default();
    if language.is_empty() {
        return false;
    }

    // 这些语法会让 syntect 的高亮状态跨越多行；按页单独渲染会丢失前文状态。
    lines.iter().any(|line| {
        has_unclosed_c_style_block_comment(line.text)
            || line.text.contains("\"\"\"")
            || line.text.contains("'''")
            || has_unclosed_rust_raw_string_start(line.text)
    })
}

fn has_unclosed_c_style_block_comment(line: &str) -> bool {
    let Some(start) = line.find("/*") else {
        return false;
    };
    !line[start + 2..].contains("*/")
}

fn has_unclosed_rust_raw_string_start(line: &str) -> bool {
    for marker in ["r#\"", "r##\"", "r###\"", "r####\""] {
        if line.contains(marker) {
            let hashes = marker.chars().filter(|ch| *ch == '#').count();
            let closing = format!("\"{}", "#".repeat(hashes));
            if !line.contains(&closing) {
                return true;
            }
        }
    }
    false
}

fn push_block(
    blocks: &mut Vec<AssistantProjectedBlock>,
    line_cursor: &mut usize,
    build: impl FnOnce(usize) -> AssistantProjectedBlock,
) {
    let block = build(*line_cursor);
    *line_cursor = line_cursor.saturating_add(block.line_count);
    blocks.push(block);
}

fn push_optional_block(
    blocks: &mut Vec<AssistantProjectedBlock>,
    line_cursor: &mut usize,
    block: AssistantProjectedBlock,
) {
    *line_cursor = line_cursor.saturating_add(block.line_count);
    blocks.push(block);
}

fn markdown_spacing_before(
    previous_block: Option<AssistantMarkdownBlock>,
    next_block: AssistantMarkdownBlock,
) -> usize {
    usize::from(should_insert_markdown_spacing(previous_block, next_block))
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

fn render_fenced_code_page(request: FencedCodePageRender<'_>) -> Vec<Line<'static>> {
    if request.line_ranges.is_empty() {
        return vec![Line::raw("")];
    }

    let first_line = request
        .line_prefix_sums
        .partition_point(|line_start| *line_start <= request.page_start)
        .saturating_sub(1)
        .min(request.line_ranges.len().saturating_sub(1));
    let last_line = request
        .line_prefix_sums
        .partition_point(|line_start| *line_start < request.page_end)
        .saturating_sub(1)
        .min(request.line_ranges.len().saturating_sub(1));
    let first_offset = request
        .page_start
        .saturating_sub(request.line_prefix_sums[first_line]);
    let page_line_count = request.page_end.saturating_sub(request.page_start);

    let mut snippet = String::new();
    snippet.push_str(request.marker);
    snippet.push_str(&request.source[request.info_range.clone()]);
    snippet.push('\n');
    for (index, range) in request.line_ranges[first_line..=last_line]
        .iter()
        .enumerate()
    {
        if index > 0 {
            snippet.push('\n');
        }
        snippet.push_str(&request.source[range.clone()]);
    }
    snippet.push('\n');
    snippet.push_str(request.marker);

    render_markdown_lines(&snippet, request.width, request.palette)
        .into_iter()
        .skip(first_offset)
        .take(page_line_count)
        .collect()
}

fn estimated_page_bytes(lines: &[Line<'static>]) -> usize {
    std::mem::size_of::<Vec<Line<'static>>>()
        + std::mem::size_of_val(lines)
        + lines
            .iter()
            .map(|line| {
                std::mem::size_of_val(line.spans.as_slice())
                    + line
                        .spans
                        .iter()
                        .map(|span| span.content.len())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

fn paragraph_line_is_projectable(line: &str) -> bool {
    let trimmed = line.trim_start();
    !trimmed.is_empty()
        && leading_space_count(line) < 4
        && !trimmed.starts_with(['>', '|'])
        && !trimmed.contains('\t')
        && !trimmed.contains('<')
        && !trimmed.contains('|')
        && !trimmed.contains('$')
}

fn markdown_fence_opener(line: &str) -> Option<(&'static str, &str)> {
    if leading_space_count(line) > 3 {
        return None;
    }

    let trimmed = line.trim_start();
    if let Some(info) = trimmed.strip_prefix("```")
        && !info.starts_with('`')
        && !info.contains('`')
    {
        return Some(("```", info));
    }
    if let Some(info) = trimmed.strip_prefix("~~~")
        && !info.starts_with('~')
    {
        return Some(("~~~", info));
    }
    None
}

fn is_markdown_fence_closer(line: &str, marker: &str) -> bool {
    if leading_space_count(line) > 3 {
        return false;
    }

    let Some(mut rest) = line.trim_start().strip_prefix(marker) else {
        return false;
    };
    let marker_char = marker.chars().next().unwrap_or('`');
    while let Some(next) = rest.strip_prefix(marker_char) {
        rest = next;
    }
    rest.trim().is_empty()
}

fn starts_with_unsupported_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("````") || trimmed.starts_with("~~~~")
}

fn is_markdown_heading(trimmed_line: &str) -> bool {
    let marker_len = trimmed_line.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&marker_len)
        && trimmed_line
            .chars()
            .nth(marker_len)
            .is_some_and(char::is_whitespace)
}

fn is_projectable_markdown_heading(line: &str, trimmed_line: &str) -> bool {
    leading_space_count(line) <= 3 && is_markdown_heading(trimmed_line)
}

fn leading_space_count(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count()
}

fn is_markdown_list_item(trimmed_line: &str) -> bool {
    let mut chars = trimmed_line.chars();
    if matches!(chars.next(), Some('-' | '*' | '+'))
        && chars.next().is_some_and(char::is_whitespace)
    {
        return true;
    }

    is_ordered_markdown_list_item(trimmed_line)
}

fn is_ordered_markdown_list_item(trimmed_line: &str) -> bool {
    let mut digit_count = 0usize;
    let mut chars = trimmed_line.chars();
    while matches!(chars.clone().next(), Some(ch) if ch.is_ascii_digit()) {
        digit_count += 1;
        chars.next();
        if digit_count > 9 {
            return false;
        }
    }

    (1..=9).contains(&digit_count)
        && matches!(chars.next(), Some('.' | ')'))
        && chars.next().is_some_and(char::is_whitespace)
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
