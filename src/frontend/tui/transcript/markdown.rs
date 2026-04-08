use std::collections::VecDeque;

#[cfg(test)]
use std::cell::Cell;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::theme::TerminalPalette;

const DISPLAY_TAB_WIDTH: usize = 8;

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
    let width = width.max(1);
    let leading_blank_lines = count_leading_blank_lines(markdown);
    let trailing_blank_lines = count_trailing_blank_lines(markdown);
    let mut renderer = MarkdownRenderer::new(palette);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

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

    let width = width.max(1);
    let leading_blank_lines = count_leading_blank_lines(markdown);
    let trailing_blank_lines = count_trailing_blank_lines(markdown);
    let mut renderer = MarkdownRenderer::new(palette);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);

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

#[cfg(test)]
pub(crate) fn reset_render_markdown_metrics_call_count() {
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn render_markdown_metrics_call_count() -> usize {
    RENDER_MARKDOWN_METRICS_CALL_COUNT.with(Cell::get)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WrapMode {
    Prose,
    Literal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StyledChunk {
    text: String,
    style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogicalLine {
    first_prefix: Vec<StyledChunk>,
    continuation_prefix: Vec<StyledChunk>,
    chunks: Vec<StyledChunk>,
    wrap_mode: WrapMode,
    preserve_trailing_spaces: bool,
}

#[derive(Debug, Clone)]
struct OpenBlock {
    first_prefix: Vec<StyledChunk>,
    continuation_prefix: Vec<StyledChunk>,
    lines: Vec<Vec<StyledChunk>>,
    wrap_mode: WrapMode,
    preserve_trailing_spaces: bool,
}

impl OpenBlock {
    fn new(
        first_prefix: Vec<StyledChunk>,
        continuation_prefix: Vec<StyledChunk>,
        wrap_mode: WrapMode,
        preserve_trailing_spaces: bool,
    ) -> Self {
        Self {
            first_prefix,
            continuation_prefix,
            lines: vec![Vec::new()],
            wrap_mode,
            preserve_trailing_spaces,
        }
    }

    fn current_line(&self) -> &[StyledChunk] {
        self.lines.last().map(Vec::as_slice).unwrap_or_default()
    }

    fn current_line_mut(&mut self) -> &mut Vec<StyledChunk> {
        self.lines
            .last_mut()
            .expect("open block should have a line")
    }

    fn prefix_width_for_current_line(&self) -> usize {
        if self.lines.len() <= 1 {
            chunk_width(&self.first_prefix)
        } else {
            chunk_width(&self.continuation_prefix)
        }
    }

    fn append_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }

        let mut column = self.prefix_width_for_current_line() + chunk_width(self.current_line());
        for grapheme in UnicodeSegmentation::graphemes(text, true) {
            if grapheme == "\t" {
                let tab_width = tab_stop_width(column);
                push_chunk(self.current_line_mut(), " ".repeat(tab_width), style);
                column += tab_width;
                continue;
            }

            push_chunk(self.current_line_mut(), grapheme.to_string(), style);
            column += grapheme.width();
        }
    }

    fn newline(&mut self) {
        self.lines.push(Vec::new());
    }

    fn into_logical_lines(mut self) -> Vec<LogicalLine> {
        if self.wrap_mode == WrapMode::Literal
            && self.lines.iter().all(|line| chunk_width(line) == 0)
        {
            return Vec::new();
        }

        let mut lines = Vec::with_capacity(self.lines.len());
        for (index, mut chunks) in self.lines.drain(..).enumerate() {
            if !self.preserve_trailing_spaces {
                trim_trailing_space_chunks(&mut chunks);
            }

            let first_prefix = if index == 0 {
                self.first_prefix.clone()
            } else {
                self.continuation_prefix.clone()
            };

            lines.push(LogicalLine {
                first_prefix,
                continuation_prefix: self.continuation_prefix.clone(),
                chunks,
                wrap_mode: self.wrap_mode,
                preserve_trailing_spaces: self.preserve_trailing_spaces,
            });
        }

        lines
    }
}

#[derive(Debug, Clone, Default)]
struct InlineStyleState {
    emphasis_depth: usize,
    strong_depth: usize,
    strike_depth: usize,
    code_depth: usize,
}

#[derive(Debug, Clone)]
struct LinkState {
    destination: String,
    rendered_text: String,
}

#[derive(Debug, Clone)]
enum ListKind {
    Bullet,
    Ordered(usize),
}

#[derive(Debug, Clone)]
struct ListFrame {
    kind: ListKind,
    active_marker: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TableBuilder {
    rows: Vec<TableRow>,
    current_row: Option<TableRow>,
    current_cell: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct TableRow {
    cells: Vec<String>,
}

struct MarkdownRenderer {
    palette: TerminalPalette,
    output: Vec<LogicalLine>,
    current_block: Option<OpenBlock>,
    list_stack: Vec<ListFrame>,
    blockquote_depth: usize,
    inline_styles: InlineStyleState,
    link_stack: Vec<LinkState>,
    table: Option<TableBuilder>,
    in_table_head: bool,
    needs_spacing: bool,
}

impl MarkdownRenderer {
    fn new(palette: TerminalPalette) -> Self {
        Self {
            palette,
            output: Vec::new(),
            current_block: None,
            list_stack: Vec::new(),
            blockquote_depth: 0,
            inline_styles: InlineStyleState::default(),
            link_stack: Vec::new(),
            table: None,
            in_table_head: false,
            needs_spacing: false,
        }
    }

    fn render<'a>(&mut self, parser: Parser<'a>) {
        for event in parser {
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.push_text(&text),
                Event::Code(code) => self.push_inline_code(&code),
                Event::SoftBreak | Event::HardBreak => self.push_newline(),
                Event::Rule => self.push_rule(),
                Event::Html(_)
                | Event::InlineHtml(_)
                | Event::InlineMath(_)
                | Event::DisplayMath(_) => {}
                Event::TaskListMarker(done) => {
                    let marker = if done { "[x] " } else { "[ ] " };
                    self.push_text(marker);
                }
                Event::FootnoteReference(text) => self.push_text(&text),
            }
        }

        self.flush_current_block();
        if let Some(table) = self.table.take() {
            self.push_table(table);
        }
    }

    fn finish(mut self, width: usize) -> Vec<Line<'static>> {
        self.output
            .drain(..)
            .flat_map(|line| wrap_logical_line(line, width.max(1)))
            .collect()
    }

    fn finish_metrics(mut self, width: usize) -> (usize, usize) {
        self.output
            .drain(..)
            .map(|line| measure_wrapped_logical_line(line, width.max(1)))
            .fold(
                (0, 0),
                |(line_count, plain_text_len), (next_lines, next_len)| {
                    (line_count + next_lines, plain_text_len + next_len)
                },
            )
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.start_prose_block(),
            Tag::Heading { level, .. } => self.start_heading_block(level),
            Tag::BlockQuote(_) => {
                self.flush_current_block();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(_) => self.start_literal_block(true),
            Tag::List(Some(start)) => self.list_stack.push(ListFrame {
                kind: ListKind::Ordered(start as usize),
                active_marker: None,
            }),
            Tag::List(None) => self.list_stack.push(ListFrame {
                kind: ListKind::Bullet,
                active_marker: None,
            }),
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self.inline_styles.emphasis_depth += 1,
            Tag::Strong => self.inline_styles.strong_depth += 1,
            Tag::Strikethrough => self.inline_styles.strike_depth += 1,
            Tag::Link { dest_url, .. } => self.link_stack.push(LinkState {
                destination: dest_url.to_string(),
                rendered_text: String::new(),
            }),
            Tag::Image { dest_url, .. } => self.link_stack.push(LinkState {
                destination: dest_url.to_string(),
                rendered_text: String::new(),
            }),
            Tag::Table(_) => {
                self.flush_current_block();
                self.table = Some(TableBuilder::default());
            }
            Tag::TableHead => self.in_table_head = true,
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.current_row = Some(TableRow::default());
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.current_cell = Some(String::new());
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock => {
                self.flush_current_block();
                self.needs_spacing = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_current_block();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::List(_) => {
                self.flush_current_block();
                self.list_stack.pop();
            }
            TagEnd::Item => {
                self.flush_current_block();
                if let Some(frame) = self.list_stack.last_mut() {
                    frame.active_marker = None;
                }
            }
            TagEnd::Emphasis => {
                self.inline_styles.emphasis_depth =
                    self.inline_styles.emphasis_depth.saturating_sub(1);
            }
            TagEnd::Strong => {
                self.inline_styles.strong_depth = self.inline_styles.strong_depth.saturating_sub(1);
            }
            TagEnd::Strikethrough => {
                self.inline_styles.strike_depth = self.inline_styles.strike_depth.saturating_sub(1);
            }
            TagEnd::Link | TagEnd::Image => self.finish_link(),
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.push_table(table);
                    self.needs_spacing = true;
                }
            }
            TagEnd::TableHead => self.in_table_head = false,
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table
                    && let Some(row) = table.current_row.take()
                {
                    table.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table
                    && let Some(cell) = table.current_cell.take()
                    && let Some(row) = &mut table.current_row
                {
                    row.cells.push(cell);
                }
            }
            _ => {}
        }
    }

    fn start_prose_block(&mut self) {
        self.start_block(WrapMode::Prose, false);
    }

    fn start_heading_block(&mut self, level: HeadingLevel) {
        self.start_block(WrapMode::Prose, false);
        let marker = match level {
            HeadingLevel::H1 => "",
            HeadingLevel::H2 => "",
            HeadingLevel::H3 => "",
            HeadingLevel::H4 => "",
            HeadingLevel::H5 => "",
            HeadingLevel::H6 => "",
        };
        if !marker.is_empty() {
            self.push_text(marker);
        }
    }

    fn start_literal_block(&mut self, preserve_trailing_spaces: bool) {
        self.start_block(WrapMode::Literal, preserve_trailing_spaces);
    }

    fn start_block(&mut self, wrap_mode: WrapMode, preserve_trailing_spaces: bool) {
        self.flush_current_block();
        self.maybe_insert_spacing();
        let (first_prefix, continuation_prefix) = self.current_prefixes();
        self.current_block = Some(OpenBlock::new(
            first_prefix,
            continuation_prefix,
            wrap_mode,
            preserve_trailing_spaces,
        ));
    }

    fn maybe_insert_spacing(&mut self) {
        if self.needs_spacing && self.list_stack.is_empty() && !self.output.is_empty() {
            self.output.push(LogicalLine {
                first_prefix: Vec::new(),
                continuation_prefix: Vec::new(),
                chunks: Vec::new(),
                wrap_mode: WrapMode::Literal,
                preserve_trailing_spaces: false,
            });
        }
        self.needs_spacing = false;
    }

    fn current_prefixes(&self) -> (Vec<StyledChunk>, Vec<StyledChunk>) {
        let mut first = Vec::new();
        let mut continuation = Vec::new();

        for _ in 0..self.blockquote_depth {
            push_chunk(
                &mut first,
                String::from("> "),
                self.secondary_style().add_modifier(Modifier::BOLD),
            );
            push_chunk(
                &mut continuation,
                String::from("> "),
                self.secondary_style().add_modifier(Modifier::BOLD),
            );
        }

        for (index, frame) in self.list_stack.iter().enumerate() {
            let is_last = index + 1 == self.list_stack.len();
            let Some(marker) = &frame.active_marker else {
                continue;
            };

            let indent = " ".repeat(measure_width(marker));
            if is_last {
                push_chunk(&mut first, marker.clone(), self.secondary_style());
            } else {
                push_chunk(&mut first, indent.clone(), Style::new());
            }
            push_chunk(&mut continuation, indent, Style::new());
        }

        (first, continuation)
    }

    fn start_list_item(&mut self) {
        self.flush_current_block();
        if let Some(frame) = self.list_stack.last_mut() {
            let marker = match &mut frame.kind {
                ListKind::Bullet => String::from("- "),
                ListKind::Ordered(next) => {
                    let marker = format!("{next}. ");
                    *next += 1;
                    marker
                }
            };
            frame.active_marker = Some(marker);
        }
    }

    fn push_text(&mut self, text: &str) {
        if let Some(table) = &mut self.table
            && let Some(cell) = &mut table.current_cell
        {
            cell.push_str(text);
            return;
        }

        if self.current_block.is_none() {
            self.start_prose_block();
        }

        let style = self.current_text_style();
        if let Some(link) = self.link_stack.last_mut() {
            link.rendered_text.push_str(text);
        }
        if let Some(block) = &mut self.current_block {
            block.append_text(text, style);
        }
    }

    fn push_inline_code(&mut self, code: &str) {
        self.inline_styles.code_depth += 1;
        self.push_text(code);
        self.inline_styles.code_depth = self.inline_styles.code_depth.saturating_sub(1);
    }

    fn push_newline(&mut self) {
        if let Some(table) = &mut self.table
            && let Some(cell) = &mut table.current_cell
        {
            if !cell.ends_with(' ') {
                cell.push(' ');
            }
            return;
        }

        if self.current_block.is_none() {
            self.start_prose_block();
        }

        if let Some(block) = &mut self.current_block {
            block.newline();
        }
    }

    fn push_rule(&mut self) {
        self.flush_current_block();
        self.maybe_insert_spacing();
        self.output.push(LogicalLine {
            first_prefix: Vec::new(),
            continuation_prefix: Vec::new(),
            chunks: vec![StyledChunk {
                text: String::from("---"),
                style: self.secondary_style(),
            }],
            wrap_mode: WrapMode::Literal,
            preserve_trailing_spaces: false,
        });
        self.needs_spacing = true;
    }

    fn finish_link(&mut self) {
        let Some(link) = self.link_stack.pop() else {
            return;
        };

        let destination = link
            .destination
            .trim_matches(|character| character == '<' || character == '>');
        if destination.is_empty() {
            return;
        }

        if normalize_space(&link.rendered_text) == normalize_space(destination) {
            return;
        }

        let suffix = format!(" ({destination})");
        let suffix_style = self.secondary_style().add_modifier(Modifier::UNDERLINED);
        if let Some(block) = &mut self.current_block {
            block.append_text(&suffix, suffix_style);
        }
    }

    fn push_table(&mut self, table: TableBuilder) {
        if table.rows.is_empty() {
            return;
        }

        self.maybe_insert_spacing();
        for row in table.rows {
            let mut chunks = Vec::new();
            for (index, cell) in row.cells.into_iter().enumerate() {
                if index > 0 {
                    push_chunk(&mut chunks, String::from(" | "), self.secondary_style());
                }
                push_chunk(&mut chunks, cell, self.base_text_style());
            }

            trim_trailing_space_chunks(&mut chunks);
            self.output.push(LogicalLine {
                first_prefix: Vec::new(),
                continuation_prefix: Vec::new(),
                chunks,
                wrap_mode: WrapMode::Prose,
                preserve_trailing_spaces: false,
            });
        }
    }

    fn flush_current_block(&mut self) {
        let Some(block) = self.current_block.take() else {
            return;
        };
        self.output.extend(block.into_logical_lines());
    }

    fn current_text_style(&self) -> Style {
        let mut style = self.base_text_style();

        if self.inline_styles.strong_depth > 0 {
            style = style.add_modifier(Modifier::BOLD);
        }
        if self.inline_styles.emphasis_depth > 0 {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if self.inline_styles.strike_depth > 0 {
            style = style.add_modifier(Modifier::CROSSED_OUT);
        }
        if self.inline_styles.code_depth > 0 {
            style = self.code_style();
        }

        if !self.link_stack.is_empty() {
            style = style.add_modifier(Modifier::UNDERLINED);
        }

        style
    }

    fn base_text_style(&self) -> Style {
        if self.palette.uses_terminal_default_colors() {
            Style::new()
        } else {
            Style::new().fg(self.palette.main)
        }
    }

    fn secondary_style(&self) -> Style {
        if self.palette.uses_terminal_default_colors() {
            Style::new()
        } else {
            Style::new().fg(self.palette.secondary)
        }
    }

    fn code_style(&self) -> Style {
        if self.palette.uses_terminal_default_colors() {
            Style::new()
        } else {
            let mut style = Style::new().fg(self.palette.main);
            if let Some(surface) = self.palette.surface {
                style = style.bg(surface);
            }
            style
        }
    }
}

fn wrap_logical_line(line: LogicalLine, width: usize) -> Vec<Line<'static>> {
    if line.chunks.is_empty() && line.first_prefix.is_empty() {
        return vec![Line::raw("")];
    }

    let first_width = width.saturating_sub(chunk_width(&line.first_prefix)).max(1);
    let continuation_width = width
        .saturating_sub(chunk_width(&line.continuation_prefix))
        .max(1);

    let wrapped_content = match line.wrap_mode {
        WrapMode::Prose => wrap_prose_chunks(&line.chunks, first_width, continuation_width),
        WrapMode::Literal => hard_wrap_chunks(&line.chunks, first_width, continuation_width),
    };

    wrapped_content
        .into_iter()
        .enumerate()
        .map(|(index, chunks)| {
            let mut spans = Vec::new();
            let prefix = if index == 0 {
                &line.first_prefix
            } else {
                &line.continuation_prefix
            };

            for chunk in prefix.iter().chain(chunks.iter()) {
                spans.push(Span::styled(chunk.text.clone(), chunk.style));
            }

            Line::from(spans)
        })
        .collect()
}

fn measure_wrapped_logical_line(line: LogicalLine, width: usize) -> (usize, usize) {
    if line.chunks.is_empty() && line.first_prefix.is_empty() {
        return (1, 0);
    }

    let first_width = width.saturating_sub(chunk_width(&line.first_prefix)).max(1);
    let continuation_width = width
        .saturating_sub(chunk_width(&line.continuation_prefix))
        .max(1);

    let wrapped_content = match line.wrap_mode {
        WrapMode::Prose => wrap_prose_chunks(&line.chunks, first_width, continuation_width),
        WrapMode::Literal => hard_wrap_chunks(&line.chunks, first_width, continuation_width),
    };

    wrapped_content.into_iter().enumerate().fold(
        (0, 0),
        |(line_count, plain_text_len), (index, chunks)| {
            let prefix = if index == 0 {
                &line.first_prefix
            } else {
                &line.continuation_prefix
            };

            (
                line_count + 1,
                plain_text_len + chunk_text_len(prefix) + chunk_text_len(&chunks),
            )
        },
    )
}

#[derive(Debug, Clone)]
struct StyledSegment {
    text: String,
    style: Style,
    width: usize,
    is_space: bool,
}

fn wrap_prose_chunks(
    chunks: &[StyledChunk],
    first_width: usize,
    continuation_width: usize,
) -> Vec<Vec<StyledChunk>> {
    let segments = tokenize_chunks(chunks);
    if segments.is_empty() {
        return vec![Vec::new()];
    }

    let mut cursor = VecDeque::from(segments);
    let mut lines = Vec::new();
    let mut current_width = first_width.max(1);

    while !cursor.is_empty() {
        lines.push(consume_prose_line(&mut cursor, current_width));
        current_width = continuation_width.max(1);
    }

    if lines.is_empty() {
        vec![Vec::new()]
    } else {
        lines
    }
}

fn consume_prose_line(cursor: &mut VecDeque<StyledSegment>, width: usize) -> Vec<StyledChunk> {
    let mut line = Vec::new();
    let mut line_width = 0;
    let mut pending_spaces = Vec::new();
    let mut pending_space_width = 0;

    while let Some(segment) = cursor.pop_front() {
        if segment.is_space {
            if line_width == 0 {
                if segment.width <= width {
                    push_chunk(&mut line, segment.text, segment.style);
                    line_width += segment.width;
                } else {
                    let (fitted, overflow) = split_segment_to_width(segment, width);
                    push_chunk(&mut line, fitted.text, fitted.style);
                    if overflow.width > 0 {
                        cursor.push_front(overflow);
                    }
                }
                continue;
            }

            pending_space_width += segment.width;
            pending_spaces.push(segment);
            continue;
        }

        if line_width == 0 {
            if segment.width <= width {
                push_chunk(&mut line, segment.text, segment.style);
                line_width += segment.width;
            } else {
                let (fitted, overflow) = split_segment_to_width(segment, width);
                push_chunk(&mut line, fitted.text, fitted.style);
                if overflow.width > 0 {
                    cursor.push_front(overflow);
                }
            }
            continue;
        }

        if line_width + pending_space_width + segment.width <= width {
            for space in pending_spaces.drain(..) {
                push_chunk(&mut line, space.text, space.style);
            }
            pending_space_width = 0;
            push_chunk(&mut line, segment.text, segment.style);
            line_width = chunk_width(&line);
            continue;
        }

        cursor.push_front(segment);
        break;
    }

    trim_trailing_space_chunks(&mut line);
    line
}

fn hard_wrap_chunks(
    chunks: &[StyledChunk],
    first_width: usize,
    continuation_width: usize,
) -> Vec<Vec<StyledChunk>> {
    let mut lines = vec![Vec::new()];
    let mut widths = vec![0usize];
    let mut current_index = 0usize;
    let mut available_width = first_width.max(1);

    for chunk in chunks {
        for grapheme in UnicodeSegmentation::graphemes(chunk.text.as_str(), true) {
            let grapheme_width = measure_width(grapheme);
            if widths[current_index] > 0 && widths[current_index] + grapheme_width > available_width
            {
                lines.push(Vec::new());
                widths.push(0);
                current_index += 1;
                available_width = continuation_width.max(1);
            }

            push_chunk(&mut lines[current_index], grapheme.to_string(), chunk.style);
            widths[current_index] += grapheme_width;
        }
    }

    if lines.is_empty() {
        vec![Vec::new()]
    } else {
        lines
    }
}

fn tokenize_chunks(chunks: &[StyledChunk]) -> Vec<StyledSegment> {
    let mut segments = Vec::new();

    for chunk in chunks {
        let mut current = String::new();
        let mut current_width = 0;
        let mut current_is_space = None;

        for grapheme in UnicodeSegmentation::graphemes(chunk.text.as_str(), true) {
            let is_space = grapheme.chars().all(char::is_whitespace);
            match current_is_space {
                Some(existing) if existing != is_space => {
                    segments.push(StyledSegment {
                        text: std::mem::take(&mut current),
                        style: chunk.style,
                        width: current_width,
                        is_space: existing,
                    });
                    current_width = 0;
                    current_is_space = Some(is_space);
                }
                None => current_is_space = Some(is_space),
                _ => {}
            }

            current.push_str(grapheme);
            current_width += grapheme.width();
        }

        if let Some(is_space) = current_is_space {
            segments.push(StyledSegment {
                text: current,
                style: chunk.style,
                width: current_width,
                is_space,
            });
        }
    }

    segments
}

fn split_segment_to_width(segment: StyledSegment, width: usize) -> (StyledSegment, StyledSegment) {
    let (fitted_text, overflow_text) = split_text_to_width(&segment.text, width);

    (
        StyledSegment {
            width: measure_width(&fitted_text),
            text: fitted_text,
            style: segment.style,
            is_space: segment.is_space,
        },
        StyledSegment {
            width: measure_width(&overflow_text),
            text: overflow_text,
            style: segment.style,
            is_space: segment.is_space,
        },
    )
}

fn split_text_to_width(text: &str, width: usize) -> (String, String) {
    if text.is_empty() || width == 0 {
        return (String::new(), text.to_string());
    }

    let mut fitted = String::new();
    let mut current_width = 0;
    let mut byte_offset = 0;

    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = measure_width(grapheme);
        if current_width > 0 && current_width + grapheme_width > width {
            break;
        }

        fitted.push_str(grapheme);
        current_width += grapheme_width;
        byte_offset += grapheme.len();
    }

    if byte_offset == 0 {
        return (text.to_string(), String::new());
    }

    (fitted, text[byte_offset..].to_string())
}

fn push_chunk(chunks: &mut Vec<StyledChunk>, text: impl Into<String>, style: Style) {
    let text = text.into();
    if text.is_empty() {
        return;
    }

    if let Some(last) = chunks.last_mut()
        && last.style == style
    {
        last.text.push_str(&text);
        return;
    }

    chunks.push(StyledChunk { text, style });
}

fn trim_trailing_space_chunks(chunks: &mut Vec<StyledChunk>) {
    while let Some(last) = chunks.last_mut() {
        let trimmed = last.text.trim_end_matches(char::is_whitespace);
        if trimmed.len() == last.text.len() {
            break;
        }

        if trimmed.is_empty() {
            chunks.pop();
            continue;
        }

        last.text.truncate(trimmed.len());
        break;
    }
}

fn tab_stop_width(column: usize) -> usize {
    let mut tab_width = DISPLAY_TAB_WIDTH - (column % DISPLAY_TAB_WIDTH);
    if tab_width == 0 {
        tab_width = DISPLAY_TAB_WIDTH;
    }
    tab_width
}

fn chunk_width(chunks: &[StyledChunk]) -> usize {
    chunks.iter().map(|chunk| measure_width(&chunk.text)).sum()
}

fn chunk_text_len(chunks: &[StyledChunk]) -> usize {
    chunks.iter().map(|chunk| chunk.text.len()).sum()
}

fn measure_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn count_leading_blank_lines(markdown: &str) -> usize {
    markdown
        .split('\n')
        .take_while(|line| line.is_empty())
        .count()
}

fn count_trailing_blank_lines(markdown: &str) -> usize {
    markdown
        .rsplit('\n')
        .take_while(|line| line.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::render_markdown_lines;
    use crate::frontend::tui::{
        styled_text::{lines_to_ansi_text, lines_to_plain_text},
        theme::{default_palette, terminal_default_palette},
    };

    #[test]
    fn render_markdown_removes_heading_markers() {
        let lines = render_markdown_lines("# Overview of the API", 20, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "Overview of the API");
    }

    #[test]
    fn render_markdown_removes_emphasis_markers() {
        let lines = render_markdown_lines("__init__", 20, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "init");
    }

    #[test]
    fn render_markdown_renders_fenced_code_without_fence_markers() {
        let lines = render_markdown_lines(
            "```go\nif err != nil {\n\treturn err\n}\n```",
            20,
            default_palette(),
        );
        let rendered = lines_to_plain_text(&lines);

        assert!(!rendered.contains("```"));
        assert!(rendered.contains("if err != nil {"));
        assert!(rendered.contains("return err"));
    }

    #[test]
    fn render_markdown_preserves_link_destinations() {
        let lines = render_markdown_lines("[main.go](<cmd/lumos/main.go>)", 40, default_palette());
        let rendered = lines_to_plain_text(&lines);

        assert!(rendered.contains("cmd/lumos/main.go"));
    }

    #[test]
    fn render_markdown_keeps_terminal_default_plain_text_unstyled() {
        let lines = render_markdown_lines("plain text", 20, terminal_default_palette());
        let rendered = lines_to_ansi_text(&lines);

        assert_eq!(rendered, "plain text");
    }

    #[test]
    fn render_markdown_preserves_explicit_edge_blank_lines() {
        let lines = render_markdown_lines("\nhello\n", 20, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "\nhello\n");
    }

    #[test]
    fn render_markdown_does_not_insert_blank_row_before_wide_glyph() {
        let lines = render_markdown_lines("中", 1, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "中");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn render_markdown_perf_smoke() {
        use std::hint::black_box;

        let markdown = (0..6)
            .map(|index| {
                format!(
                    "## Section {index}\n\n- summarize the latest transcript cache behavior\n- explain why viewport anchors stay stable across resize\n- keep the markdown renderer width-aware\n\n```rust\nfn section_{index}() -> Result<(), &'static str> {{\n    Ok(())\n}}\n```\n"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        for _ in 0..128 {
            black_box(render_markdown_lines(&markdown, 72, default_palette()));
        }
    }
}
