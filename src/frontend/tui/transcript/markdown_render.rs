use std::collections::VecDeque;
use std::path::{Path, PathBuf};

#[cfg(test)]
use std::cell::Cell;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::theme::TerminalPalette;
use crate::frontend::tui::transcript::markdown_highlight::highlight_code_chunks;
use crate::frontend::tui::transcript::markdown_links::render_local_link_target;
use crate::frontend::tui::transcript::markdown_table::{
    MarkdownTable, TableCellKind, TableLine, render_markdown_table,
};

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
    let mut renderer = MarkdownRenderer::new(palette, cwd.as_deref(), width);
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

        for segment in text.split_inclusive('\n') {
            let mut line_text = segment.strip_suffix('\n').unwrap_or(segment);
            if let Some(stripped) = line_text.strip_suffix('\r') {
                line_text = stripped;
            }
            self.append_text_without_newlines(line_text, style);
            if segment.ends_with('\n') {
                self.newline();
            }
        }
    }

    fn append_text_without_newlines(&mut self, text: &str, style: Style) {
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

    fn append_styled_lines(&mut self, lines: Vec<Vec<StyledChunk>>) {
        for (index, line) in lines.into_iter().enumerate() {
            if index > 0 || chunk_width(self.current_line()) > 0 {
                self.newline();
            }
            self.current_line_mut().extend(line);
        }
    }

    fn newline(&mut self) {
        self.lines.push(Vec::new());
    }

    fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| chunk_width(line) == 0)
    }

    fn into_logical_lines(mut self) -> Vec<LogicalLine> {
        if self.wrap_mode == WrapMode::Literal
            && self.lines.iter().all(|line| chunk_width(line) == 0)
        {
            return Vec::new();
        }

        if self.wrap_mode == WrapMode::Literal
            && self.lines.last().is_some_and(|line| chunk_width(line) == 0)
            && self.lines.len() > 1
        {
            self.lines.pop();
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
    heading_style: Option<Style>,
}

#[derive(Debug, Clone)]
struct LinkState {
    destination: String,
    rendered_text: String,
    local_target_display: Option<String>,
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
    continuation_indent: String,
}

#[derive(Debug, Clone, Default)]
struct TableBuilder {
    alignments: Vec<Alignment>,
    header: Vec<String>,
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
    cwd: Option<PathBuf>,
    width: usize,
    output: Vec<LogicalLine>,
    current_block: Option<OpenBlock>,
    list_stack: Vec<ListFrame>,
    blockquote_depth: usize,
    inline_styles: InlineStyleState,
    link_stack: Vec<LinkState>,
    table: Option<TableBuilder>,
    in_table_head: bool,
    code_block_lang: Option<String>,
    code_block_buffer: String,
    line_ends_with_local_link_target: bool,
    pending_local_link_soft_break: bool,
    needs_spacing: bool,
}

impl MarkdownRenderer {
    fn new(palette: TerminalPalette, cwd: Option<&Path>, width: usize) -> Self {
        Self {
            palette,
            cwd: cwd.map(Path::to_path_buf),
            width: width.max(1),
            output: Vec::new(),
            current_block: None,
            list_stack: Vec::new(),
            blockquote_depth: 0,
            inline_styles: InlineStyleState::default(),
            link_stack: Vec::new(),
            table: None,
            in_table_head: false,
            code_block_lang: None,
            code_block_buffer: String::new(),
            line_ends_with_local_link_target: false,
            pending_local_link_soft_break: false,
            needs_spacing: false,
        }
    }

    fn render<'a>(&mut self, parser: Parser<'a>) {
        for event in parser {
            self.prepare_for_event(&event);
            match event {
                Event::Start(tag) => self.start_tag(tag),
                Event::End(tag) => self.end_tag(tag),
                Event::Text(text) => self.push_text(&text),
                Event::Code(code) => self.push_inline_code(&code),
                Event::SoftBreak => self.push_soft_break(),
                Event::HardBreak => self.push_hard_break(),
                Event::Rule => self.push_rule(),
                Event::Html(html) => self.push_html(&html, false),
                Event::InlineHtml(html) => self.push_html(&html, true),
                Event::InlineMath(math) => self.push_inline_math(&math),
                Event::DisplayMath(math) => self.push_display_math(&math),
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

    fn prepare_for_event(&mut self, event: &Event<'_>) {
        if !self.pending_local_link_soft_break {
            return;
        }

        if matches!(event, Event::Text(text) if text.trim_start().starts_with(':')) {
            self.pending_local_link_soft_break = false;
            return;
        }

        self.pending_local_link_soft_break = false;
        self.push_newline();
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
            Tag::CodeBlock(kind) => self.start_code_block(kind),
            Tag::List(Some(start)) => self.list_stack.push(ListFrame {
                kind: ListKind::Ordered(start as usize),
                active_marker: None,
                continuation_indent: String::new(),
            }),
            Tag::List(None) => self.list_stack.push(ListFrame {
                kind: ListKind::Bullet,
                active_marker: None,
                continuation_indent: String::new(),
            }),
            Tag::Item => self.start_list_item(),
            Tag::Emphasis => self.inline_styles.emphasis_depth += 1,
            Tag::Strong => self.inline_styles.strong_depth += 1,
            Tag::Strikethrough => self.inline_styles.strike_depth += 1,
            Tag::Link { dest_url, .. } => self.link_stack.push(LinkState {
                destination: dest_url.to_string(),
                rendered_text: String::new(),
                local_target_display: render_local_link_target(&dest_url, self.cwd.as_deref()),
            }),
            Tag::Image { dest_url, .. } => self.link_stack.push(LinkState {
                destination: dest_url.to_string(),
                rendered_text: String::new(),
                local_target_display: render_local_link_target(&dest_url, self.cwd.as_deref()),
            }),
            Tag::Table(alignments) => {
                self.flush_current_block();
                self.table = Some(TableBuilder {
                    alignments,
                    ..TableBuilder::default()
                });
            }
            Tag::TableHead => {
                self.in_table_head = true;
                if let Some(table) = &mut self.table {
                    table.current_row = Some(TableRow::default());
                }
            }
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
            TagEnd::Paragraph => {
                self.flush_current_block();
                self.needs_spacing = true;
            }
            TagEnd::CodeBlock => self.end_code_block(),
            TagEnd::Heading(_) => {
                self.flush_current_block();
                self.inline_styles.heading_style = None;
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
                    frame.continuation_indent.clear();
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
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table
                    && let Some(row) = table.current_row.take()
                {
                    table.header = row.cells;
                }
                self.in_table_head = false;
            }
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
        self.inline_styles.heading_style = Some(heading_style(level));
        self.push_text(&format!("{} ", "#".repeat(heading_level_number(level))));
    }

    fn start_literal_block(&mut self, preserve_trailing_spaces: bool) {
        self.start_block(WrapMode::Literal, preserve_trailing_spaces);
    }

    fn start_code_block(&mut self, kind: CodeBlockKind<'_>) {
        let lang = match kind {
            CodeBlockKind::Fenced(info) => info
                .split([' ', '\t', ','])
                .next()
                .map(str::trim)
                .filter(|lang| !lang.is_empty())
                .map(str::to_string),
            CodeBlockKind::Indented => None,
        };

        self.start_literal_block(true);
        self.code_block_lang = lang;
        self.code_block_buffer.clear();
    }

    fn end_code_block(&mut self) {
        if let Some(lang) = self.code_block_lang.take() {
            let code = std::mem::take(&mut self.code_block_buffer);
            let code_style = self.code_style();
            let highlighted = highlight_code_chunks(&code, &lang, self.highlighted_code_style())
                .map(|lines| {
                    lines
                        .into_iter()
                        .map(|line| {
                            line.into_iter()
                                .map(|chunk| StyledChunk {
                                    text: chunk.text,
                                    style: chunk.style,
                                })
                                .collect::<Vec<_>>()
                        })
                        .collect::<Vec<_>>()
                });

            if let Some(block) = &mut self.current_block {
                if let Some(lines) = highlighted {
                    block.append_styled_lines(lines);
                    if code.ends_with('\n') {
                        block.newline();
                    }
                } else {
                    block.append_text(&code, code_style);
                }
            }
        }

        self.flush_current_block();
        self.needs_spacing = true;
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
        self.clear_active_list_marker();
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

        if let Some(frame) = self.list_stack.last()
            && (!frame.continuation_indent.is_empty() || frame.active_marker.is_some())
        {
            let indent = if frame.continuation_indent.is_empty() {
                frame
                    .active_marker
                    .as_ref()
                    .map(|marker| " ".repeat(measure_width(marker)))
                    .unwrap_or_default()
            } else {
                frame.continuation_indent.clone()
            };

            if let Some(marker) = &frame.active_marker {
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
        let depth = self.list_stack.len().max(1);
        if let Some(frame) = self.list_stack.last_mut() {
            let marker = match &mut frame.kind {
                ListKind::Bullet => format!("{}- ", " ".repeat(depth.saturating_sub(1) * 4)),
                ListKind::Ordered(next) => {
                    let marker = format!("{next:width$}. ", width = depth * 4 - 3);
                    *next += 1;
                    marker
                }
            };
            frame.continuation_indent = " ".repeat(measure_width(&marker));
            frame.active_marker = Some(marker);
        }
    }

    fn clear_active_list_marker(&mut self) {
        if let Some(frame) = self.list_stack.last_mut() {
            frame.active_marker = None;
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.code_block_lang.is_some() {
            self.code_block_buffer.push_str(text);
            return;
        }

        let suppress_local_link_label = self
            .link_stack
            .last()
            .and_then(|link| link.local_target_display.as_ref())
            .is_some();
        if let Some(link) = self.link_stack.last_mut() {
            link.rendered_text.push_str(text);
        }
        if suppress_local_link_label {
            return;
        }

        self.line_ends_with_local_link_target = false;

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
        if let Some(block) = &mut self.current_block {
            block.append_text(text, style);
        }
    }

    fn push_inline_code(&mut self, code: &str) {
        self.inline_styles.code_depth += 1;
        self.push_text(code);
        self.inline_styles.code_depth = self.inline_styles.code_depth.saturating_sub(1);
    }

    fn push_inline_math(&mut self, math: &str) {
        self.push_inline_code(math);
    }

    fn push_display_math(&mut self, math: &str) {
        if self.current_block.as_ref().is_some_and(OpenBlock::is_empty) {
            self.current_block = None;
        } else {
            self.flush_current_block();
        }
        self.start_literal_block(true);

        let math = trim_display_math_text(math);
        let style = self.code_style();
        if let Some(block) = &mut self.current_block {
            block.append_text(math, style);
        }

        self.flush_current_block();
        self.needs_spacing = true;
    }

    fn push_soft_break(&mut self) {
        if self.line_ends_with_local_link_target {
            self.pending_local_link_soft_break = true;
            self.line_ends_with_local_link_target = false;
            return;
        }

        self.push_newline();
    }

    fn push_hard_break(&mut self) {
        self.line_ends_with_local_link_target = false;
        self.pending_local_link_soft_break = false;
        self.push_newline();
    }

    fn push_html(&mut self, html: &str, inline: bool) {
        if inline {
            self.push_text(html);
            return;
        }

        self.flush_current_block();
        self.start_literal_block(false);
        let style = self.current_text_style();
        if let Some(block) = &mut self.current_block {
            block.append_text(html, style);
        }
        self.flush_current_block();
        self.needs_spacing = true;
    }

    fn push_newline(&mut self) {
        self.line_ends_with_local_link_target = false;

        if self.code_block_lang.is_some() {
            self.code_block_buffer.push('\n');
            return;
        }

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

        if let Some(local_target_display) = link.local_target_display {
            if let Some(table) = &mut self.table
                && let Some(cell) = &mut table.current_cell
            {
                cell.push_str(&local_target_display);
                return;
            }

            if self.current_block.is_none() {
                self.start_prose_block();
            }

            let style = self.code_style();
            if let Some(block) = &mut self.current_block {
                block.append_text(&local_target_display, style);
            }
            self.line_ends_with_local_link_target = true;
            return;
        }

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
        if let Some(table) = &mut self.table
            && let Some(cell) = &mut table.current_cell
        {
            cell.push_str(&suffix);
            return;
        }

        let suffix_style = self.secondary_style().add_modifier(Modifier::UNDERLINED);
        if let Some(block) = &mut self.current_block {
            block.append_text(&suffix, suffix_style);
        }
    }

    fn push_table(&mut self, table: TableBuilder) {
        if table.header.is_empty() && table.rows.is_empty() {
            return;
        }

        self.maybe_insert_spacing();
        let table = MarkdownTable {
            alignments: table.alignments,
            header: table.header,
            rows: table.rows.into_iter().map(|row| row.cells).collect(),
        };

        for line in render_markdown_table(&table, self.width) {
            self.output.push(LogicalLine {
                first_prefix: Vec::new(),
                continuation_prefix: Vec::new(),
                chunks: self.table_line_chunks(line),
                wrap_mode: WrapMode::Literal,
                preserve_trailing_spaces: false,
            });
        }
    }

    fn table_line_chunks(&self, line: TableLine) -> Vec<StyledChunk> {
        let border_style = self.secondary_style();
        let body_style = self.base_text_style();
        let header_style = self.base_text_style().add_modifier(Modifier::BOLD);

        line.into_iter()
            .map(|segment| StyledChunk {
                text: segment.text,
                style: match segment.kind {
                    TableCellKind::Border => border_style,
                    TableCellKind::Header => header_style,
                    TableCellKind::Body => body_style,
                },
            })
            .collect()
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
        if let Some(heading_style) = self.inline_styles.heading_style {
            style = style.patch(heading_style);
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

    fn highlighted_code_style(&self) -> Style {
        if self.palette.uses_terminal_default_colors() {
            Style::new()
        } else {
            Style::new().fg(self.palette.main)
        }
    }
}

fn heading_level_number(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn heading_style(level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        HeadingLevel::H2 => Style::new().add_modifier(Modifier::BOLD),
        HeadingLevel::H3 => Style::new().add_modifier(Modifier::BOLD | Modifier::ITALIC),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => {
            Style::new().add_modifier(Modifier::ITALIC)
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

fn trim_display_math_text(text: &str) -> &str {
    text.trim_matches(['\n', '\r'])
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
    use ratatui::style::Modifier;

    #[test]
    fn render_markdown_uses_codex_style_heading_markers() {
        let lines = render_markdown_lines("# Overview of the API", 80, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "# Overview of the API");
    }

    #[test]
    fn render_markdown_removes_emphasis_markers() {
        let lines = render_markdown_lines("__init__", 20, default_palette());
        assert_eq!(lines_to_plain_text(&lines), "init");
    }

    #[test]
    fn render_markdown_strikethrough_applies_crossed_out_style() {
        let lines = render_markdown_lines("keep ~~drop~~ now", 80, default_palette());
        let strike_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "drop")
            .expect("strikethrough text should render as a separate styled span");

        assert_eq!(lines_to_plain_text(&lines), "keep drop now");
        assert!(
            strike_span
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT),
            "删除线文本应使用 Ratatui 的 CROSSED_OUT 样式: {strike_span:?}"
        );
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
    fn render_markdown_preserves_intentional_trailing_blank_line_in_code_block() {
        let lines = render_markdown_lines("```rust\nfn main() {}\n\n```", 80, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "fn main() {}\n");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn render_markdown_splits_embedded_text_newlines_into_real_lines() {
        let lines = render_markdown_lines(
            "# 简单文档\n这是一个示例 Markdown 文件。\n## 列表\n- 项目一\n- 项目二\n\n```rust\nfn main() {\n    println!(\"Hello, world!\");\n}\n```\n\n[示例链接](https://example.com)",
            80,
            default_palette(),
        );

        for line in &lines {
            for span in &line.spans {
                assert!(
                    !span.content.contains('\n'),
                    "Ratatui Line/Span 不能包含内嵌换行，必须提前拆成独立视觉行: {:?}",
                    span.content
                );
            }
        }

        let rendered = lines_to_plain_text(&lines);
        assert!(rendered.contains("fn main() {\n"));
        assert!(rendered.contains("println!(\"Hello, world!\");\n"));
        assert!(!rendered.contains("```rustfn main()"));
        assert!(!rendered.contains("项目一- 项目二"));
    }

    #[test]
    fn render_markdown_preserves_link_destinations() {
        let lines = render_markdown_lines("[main.go](<cmd/lumos/main.go>)", 40, default_palette());
        let rendered = lines_to_plain_text(&lines);

        assert!(rendered.contains("cmd/lumos/main.go"));
    }

    #[test]
    fn render_markdown_local_link_uses_normalized_target_not_label() {
        let cwd = std::env::current_dir().expect("test should run inside the workspace");
        let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
        let markdown = format!("[custom label](<{}:74:3-76:9>)", target.display());

        let lines = render_markdown_lines(&markdown, 120, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
        );
    }

    #[test]
    fn render_markdown_file_url_hash_location_is_normalized() {
        let cwd = std::env::current_dir().expect("test should run inside the workspace");
        let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
        let markdown = format!("[ignored](file://{}#L74C3-L76C9)", target.display());

        let lines = render_markdown_lines(&markdown, 120, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
        );
    }

    #[test]
    fn render_markdown_decodes_percent_encoded_local_link_target() {
        let cwd = std::env::current_dir().expect("test should run inside the workspace");
        let markdown = format!(
            "[report](<{}/Example%20Folder/R%C3%A9sum%C3%A9/report.md>)",
            cwd.display()
        );

        let lines = render_markdown_lines(&markdown, 120, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "Example Folder/Résumé/report.md"
        );
    }

    #[test]
    fn render_markdown_local_file_link_soft_break_before_colon_stays_inline() {
        let cwd = std::env::current_dir().expect("test should run inside the workspace");
        let target = cwd.join("README.md");
        let markdown = format!(
            "- [binary](<{}:93>)\n  : core owns the runtime behavior.",
            target.display()
        );

        let lines = render_markdown_lines(&markdown, 120, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "- README.md:93: core owns the runtime behavior."
        );
    }

    #[test]
    fn render_markdown_web_link_keeps_label_and_destination() {
        let lines = render_markdown_lines("[Example](https://example.com)", 80, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "Example (https://example.com)");
    }

    #[test]
    fn render_markdown_renders_inline_html_as_literal_text() {
        let lines = render_markdown_lines("Press <kbd>Ctrl</kbd> now", 80, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "Press <kbd>Ctrl</kbd> now");
    }

    #[test]
    fn render_markdown_renders_block_html_lines_as_literal_text() {
        let lines = render_markdown_lines(
            "<details>\n<summary>More</summary>\n</details>\n\nAfter",
            80,
            default_palette(),
        );
        let rendered = lines_to_plain_text(&lines);

        assert!(rendered.contains("<details>"));
        assert!(rendered.contains("<summary>More</summary>"));
        assert!(rendered.contains("</details>"));
        assert!(rendered.contains("After"));
    }

    #[test]
    fn render_markdown_highlights_known_fenced_code_language() {
        let lines = render_markdown_lines(
            "```rust\nfn main() { let value = 42; }\n```",
            120,
            default_palette(),
        );
        let rendered = lines_to_plain_text(&lines);

        assert_eq!(rendered, "fn main() { let value = 42; }");
        assert!(
            lines[0].spans.len() > 1,
            "known languages should produce syntax-level spans, got {:?}",
            lines[0].spans
        );
        let mut styles = lines[0]
            .spans
            .iter()
            .map(|span| span.style)
            .collect::<Vec<_>>();
        styles.sort_by_key(|style| format!("{style:?}"));
        styles.dedup();
        assert!(
            styles.len() > 1,
            "syntax highlighting should use more than one style, got {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn render_markdown_highlights_two_face_extra_language() {
        let lines = render_markdown_lines(
            "```typescript\nconst answer: number = 42;\n```",
            120,
            default_palette(),
        );

        assert_eq!(lines_to_plain_text(&lines), "const answer: number = 42;");
        assert!(
            lines[0].spans.len() > 1,
            "two_face 扩展语法集应识别 TypeScript 并产生语法级 span: {:?}",
            lines[0].spans
        );
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.bg.is_none()),
            "已识别语言的 two_face 高亮代码块不应叠加背景色: {lines:?}"
        );
    }

    #[test]
    fn render_markdown_highlighted_fenced_code_does_not_use_block_background() {
        let lines = render_markdown_lines("```rust\nfn main() {}\n```", 80, default_palette());

        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.bg.is_none()),
            "已识别语言的语法高亮代码块不应再叠加背景色: {lines:?}"
        );
    }

    #[test]
    fn render_markdown_unknown_fenced_code_language_stays_plain_text() {
        let palette = default_palette();
        let lines = render_markdown_lines("```not-a-real-language\nhello\n```", 80, palette);

        assert_eq!(lines_to_plain_text(&lines), "hello");
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].spans[0].style.bg, palette.surface,
            "未识别语言的代码块仍应保留背景色，帮助和普通正文区分"
        );
    }

    #[test]
    fn render_markdown_inline_code_keeps_code_background() {
        let palette = default_palette();
        let lines = render_markdown_lines("use `cargo test` first", 80, palette);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "cargo test")
            .expect("inline code span should render separately");

        assert_eq!(
            code_span.style.bg, palette.surface,
            "行内代码背景色不属于本次代码块背景调整范围"
        );
    }

    #[test]
    fn render_markdown_inline_math_uses_code_background() {
        let palette = default_palette();
        let lines = render_markdown_lines("energy $E = mc^2$ now", 80, palette);
        let math_span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "E = mc^2")
            .expect("inline math should render as a separate styled span");

        assert_eq!(lines_to_plain_text(&lines), "energy E = mc^2 now");
        assert_eq!(
            math_span.style.bg, palette.surface,
            "行内 math 应使用未识别语言代码块同款背景色"
        );
    }

    #[test]
    fn render_markdown_display_math_uses_literal_code_background() {
        let palette = default_palette();
        let lines = render_markdown_lines("$$\nE = mc^2\n$$", 80, palette);

        assert_eq!(lines_to_plain_text(&lines), "E = mc^2");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .all(|span| span.style.bg == palette.surface),
            "块级 math 应使用未识别语言代码块同款背景色: {lines:?}"
        );
    }

    #[test]
    fn render_markdown_does_not_enable_footnote_definitions() {
        let lines = render_markdown_lines("[^n]: note", 80, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "");
    }

    #[test]
    fn render_markdown_keeps_heading_attributes_literal() {
        let lines = render_markdown_lines("# Title {#custom-id .lead}", 80, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "# Title {#custom-id .lead}");
    }

    #[test]
    fn render_markdown_renders_tables_with_connected_box_borders() {
        let markdown = "| 名称 | 类型 | 版本 | 启用 |\n| --- | --- | ---: | :---: |\n| lumos | 应用 | 0.1.0 | 是 |\n| ratatui | 依赖 | 0.24 | 否 |";

        let lines = render_markdown_lines(markdown, 80, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "┌─────────┬──────┬───────┬──────┐\n\
             │ 名称    │ 类型 │  版本 │ 启用 │\n\
             ├─────────┼──────┼───────┼──────┤\n\
             │ lumos   │ 应用 │ 0.1.0 │  是  │\n\
             │ ratatui │ 依赖 │  0.24 │  否  │\n\
             └─────────┴──────┴───────┴──────┘"
        );
    }

    #[test]
    fn render_markdown_wraps_table_cells_in_narrow_width_without_ellipsis() {
        let markdown =
            "| 名称 | 说明 |\n| --- | --- |\n| lumos | 一个基于 Rust 和 Ratatui 的 TUI 客户端 |";

        let lines = render_markdown_lines(markdown, 24, default_palette());
        let rendered = lines_to_plain_text(&lines);

        assert!(rendered.contains("┌"));
        assert!(rendered.contains("┬"));
        assert!(rendered.contains("┼"));
        assert!(rendered.contains("┘"));
        assert!(
            rendered.contains("Ratatui"),
            "窄窗口表格必须换行保留内容，而不是省略: {rendered}"
        );
        for token in [
            "一个",
            "基于",
            "Rust",
            "和",
            "Ratatui",
            "的",
            "TUI",
            "客户端",
        ] {
            assert!(
                rendered.contains(token),
                "窄窗口表格必须完整保留 cell 内容，缺少 {token}: {rendered}"
            );
        }
        assert!(
            !rendered.contains('…'),
            "窄窗口表格不应使用省略号截断内容: {rendered}"
        );
        assert!(
            lines.len() > 5,
            "长 cell 应该增加表格行高以完整显示内容: {rendered}"
        );
    }

    #[test]
    fn render_markdown_keeps_non_table_pipe_text_plain() {
        let markdown = "苹果 | 10 | 有货\n香蕉 | 5 | 缺货";
        let lines = render_markdown_lines(markdown, 80, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "苹果 | 10 | 有货\n香蕉 | 5 | 缺货"
        );
    }

    #[test]
    fn render_markdown_renders_task_list_markers() {
        let lines = render_markdown_lines("- [x] done\n- [ ] todo", 40, default_palette());

        assert_eq!(lines_to_plain_text(&lines), "- [x] done\n- [ ] todo");
    }

    #[test]
    fn render_markdown_nested_lists_use_codex_style_indent() {
        let lines =
            render_markdown_lines("- outer\n  - inner\n    1. ordered", 80, default_palette());

        assert_eq!(
            lines_to_plain_text(&lines),
            "- outer\n    - inner\n        1. ordered"
        );
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
