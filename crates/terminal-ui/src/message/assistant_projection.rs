use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use ratatui::text::Line;
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    display_width::grapheme_width,
    markdown_source::{MarkdownSourceBounds, markdown_source_bounds},
    styled_text::{line_plain_text_len, line_to_plain_text},
    theme::TerminalPalette,
    transcript::{
        assistant_markdown_options,
        markdown_blocks::{MarkdownBlockKind, markdown_block_spacing_before},
        markdown_table_source::contains_table_structure,
        render_markdown_lines, render_markdown_metrics,
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

#[derive(Debug)]
struct ParserProjectionBlock {
    kind: ParserProjectionBlockKind,
    range: Range<usize>,
}

#[derive(Debug)]
enum ParserProjectionBlockKind {
    Heading,
    List,
    Paragraph,
    FencedCode,
}

impl ParserProjectionBlockKind {
    fn markdown_block_kind(&self) -> MarkdownBlockKind {
        match self {
            Self::Heading => MarkdownBlockKind::Heading,
            Self::List => MarkdownBlockKind::List,
            Self::Paragraph => MarkdownBlockKind::Paragraph,
            Self::FencedCode => MarkdownBlockKind::Code,
        }
    }

    fn is_markdown_structure(&self) -> bool {
        !matches!(self, Self::Paragraph)
    }
}

struct FencedCodeSourceBlock<'a> {
    marker: &'static str,
    info_range: Range<usize>,
    code_lines: Vec<AssistantSourceLine<'a>>,
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

    let source_bounds = markdown_source_bounds(content);
    if source_bounds.is_empty() {
        return None;
    }
    let leading_blank_lines = source_bounds.leading_blank_lines;
    let parser_blocks = collect_parser_projection_blocks(content, source_bounds)?;

    let mut blocks = Vec::new();
    let mut line_cursor = 0usize;
    for _ in 0..leading_blank_lines {
        push_block(&mut blocks, &mut line_cursor, |start_line| {
            AssistantProjectedBlock::blank(start_line)
        });
    }

    let mut saw_markdown_structure = false;
    let mut previous_block = None;
    for parser_block in parser_blocks {
        saw_markdown_structure |= parser_block.kind.is_markdown_structure();
        let leading_blank_lines = markdown_block_spacing_before(previous_block);
        let markdown_block_kind = parser_block.kind.markdown_block_kind();
        match parser_block.kind {
            ParserProjectionBlockKind::FencedCode => {
                let fenced_code = projectable_fenced_code_block(content, parser_block.range)?;
                let line_ranges = fenced_code
                    .code_lines
                    .iter()
                    .map(|line| line.start..line.end)
                    .collect::<Vec<_>>();
                push_block(&mut blocks, &mut line_cursor, |start_line| {
                    AssistantProjectedBlock::fenced_code(
                        start_line,
                        leading_blank_lines,
                        fenced_code.marker,
                        content,
                        fenced_code.info_range,
                        line_ranges,
                        width,
                    )
                });
            }
            ParserProjectionBlockKind::Paragraph => {
                if !markdown_range_lines_are_projectable(content, parser_block.range.clone()) {
                    return None;
                }
                let block = AssistantProjectedBlock::markdown_snippet(
                    line_cursor,
                    leading_blank_lines,
                    content,
                    parser_block.range,
                    width,
                    palette,
                )?;
                push_optional_block(&mut blocks, &mut line_cursor, block);
            }
            ParserProjectionBlockKind::Heading | ParserProjectionBlockKind::List => {
                let block = AssistantProjectedBlock::markdown_snippet(
                    line_cursor,
                    leading_blank_lines,
                    content,
                    parser_block.range,
                    width,
                    palette,
                )?;
                push_optional_block(&mut blocks, &mut line_cursor, block);
            }
        }
        previous_block = Some(markdown_block_kind);
    }

    if !saw_markdown_structure {
        return None;
    }

    for _ in 0..source_bounds.trailing_blank_lines {
        push_block(&mut blocks, &mut line_cursor, |start_line| {
            AssistantProjectedBlock::blank(start_line)
        });
    }

    Some(blocks)
}

fn collect_parser_projection_blocks(
    content: &str,
    source_bounds: MarkdownSourceBounds,
) -> Option<Vec<ParserProjectionBlock>> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;

    // projection 的顶层 block 边界必须跟 eager renderer 同源，避免重新实现
    // CommonMark list/paragraph/fence 归属规则；不支持的 parser block 保守回退。
    for (event, range) in Parser::new_ext(content, assistant_markdown_options()).into_offset_iter()
    {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let kind = match tag {
                        Tag::Paragraph => ParserProjectionBlockKind::Paragraph,
                        Tag::Heading { .. } => ParserProjectionBlockKind::Heading,
                        Tag::List(_) => ParserProjectionBlockKind::List,
                        Tag::CodeBlock(CodeBlockKind::Fenced(_)) => {
                            ParserProjectionBlockKind::FencedCode
                        }
                        _ => return None,
                    };
                    blocks.push(ParserProjectionBlock {
                        kind,
                        range: trim_parser_block_range(content, range, source_bounds)?,
                    });
                }
                depth = depth.saturating_add(1);
            }
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
            }
            Event::Rule | Event::Html(_) | Event::InlineHtml(_) | Event::DisplayMath(_) => {
                return None;
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::InlineMath(_)
            | Event::TaskListMarker(_)
            | Event::FootnoteReference(_) => {}
        }
    }

    (!blocks.is_empty()).then_some(blocks)
}

fn trim_parser_block_range(
    content: &str,
    range: Range<usize>,
    source_bounds: MarkdownSourceBounds,
) -> Option<Range<usize>> {
    let bounded_start = range.start.max(source_bounds.content_start);
    let bounded_end = range.end.min(source_bounds.content_end);
    if bounded_start >= bounded_end {
        return None;
    }

    // pulldown-cmark 的 block range 可能包含分隔用的行尾换行。这里复用
    // Markdown source bounds 去掉 block 外层空白，block 间距仍只由共享 spacing
    // policy 注入，避免 snippet 自身 trailing blank 与 transition spacing 叠加。
    let block_bounds = markdown_source_bounds(&content[bounded_start..bounded_end]);
    if block_bounds.is_empty() {
        return None;
    }

    Some(bounded_start + block_bounds.content_start..bounded_start + block_bounds.content_end)
}

fn collect_source_lines(content: &str) -> Vec<AssistantSourceLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;

    // `split_inclusive('\n')` 覆盖无结尾换行的最后片段，避免 projection
    // 和 Markdown source bounds 维护两套尾段扫描语义。
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

    lines
}

fn collect_source_lines_in_range(
    content: &str,
    range: Range<usize>,
) -> Vec<AssistantSourceLine<'_>> {
    collect_source_lines(&content[range.clone()])
        .into_iter()
        .map(|line| AssistantSourceLine {
            text: line.text,
            start: range.start + line.start,
            end: range.start + line.end,
        })
        .collect()
}

fn projectable_fenced_code_block(
    content: &str,
    range: Range<usize>,
) -> Option<FencedCodeSourceBlock<'_>> {
    let source_lines = collect_source_lines_in_range(content, range);
    let opener = source_lines.first().copied()?;
    let (marker, info_range) = markdown_fence_opener(opener)?;
    let closer = source_lines.last().copied()?;
    if !is_markdown_fence_closer(closer.text, marker) {
        return None;
    }
    let code_lines = source_lines
        .get(1..source_lines.len().checked_sub(1)?)
        .unwrap_or_default()
        .to_vec();
    if code_lines.iter().all(|line| line.text.is_empty()) {
        return None;
    }
    if fenced_code_requires_full_context(&content[info_range.clone()], &code_lines) {
        return None;
    }

    Some(FencedCodeSourceBlock {
        marker,
        info_range,
        code_lines,
    })
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

fn markdown_range_lines_are_projectable(content: &str, range: Range<usize>) -> bool {
    collect_source_lines_in_range(content, range)
        .iter()
        .all(|line| {
            let trimmed = line.text.trim_start();
            !trimmed.is_empty()
                && leading_space_count(line.text) < 4
                && !trimmed.starts_with(['>', '|'])
                && !trimmed.contains('<')
                && !trimmed.contains('|')
                && !trimmed.contains('$')
        })
}

fn markdown_fence_opener(line: AssistantSourceLine<'_>) -> Option<(&'static str, Range<usize>)> {
    let line_text = line.text;
    if leading_space_count(line_text) > 3 {
        return None;
    }

    let trimmed = line_text.trim_start();
    let indent_len = line_text.len().saturating_sub(trimmed.len());
    if let Some(info) = trimmed.strip_prefix("```")
        && !info.starts_with('`')
        && !info.contains('`')
    {
        let info_start = line.start + indent_len + "```".len();
        return Some(("```", info_start..line.end));
    }
    if let Some(info) = trimmed.strip_prefix("~~~")
        && !info.starts_with('~')
    {
        let info_start = line.start + indent_len + "~~~".len();
        return Some(("~~~", info_start..line.end));
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

fn leading_space_count(line: &str) -> usize {
    line.as_bytes()
        .iter()
        .take_while(|byte| **byte == b' ')
        .count()
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
