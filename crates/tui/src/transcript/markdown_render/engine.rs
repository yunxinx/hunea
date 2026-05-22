use std::path::{Path, PathBuf};

use crate::{
    theme::{TerminalPalette, quote_text_style},
    transcript::{
        markdown_highlight::highlight_code_chunks,
        markdown_links::render_local_link_target,
        markdown_table::{MarkdownTable, TableCellKind, TableLine, render_markdown_table},
    },
};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use super::wrapping::{
    LogicalLine, OpenBlock, StyledChunk, WrapMode, measure_width, measure_wrapped_logical_line,
    normalize_space, push_chunk, trim_display_math_text, wrap_logical_line,
};

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

pub(super) struct MarkdownRenderer {
    palette: TerminalPalette,
    cwd: Option<PathBuf>,
    width: usize,
    should_highlight_code: bool,
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
    pub(super) fn new(palette: TerminalPalette, cwd: Option<&Path>, width: usize) -> Self {
        Self::new_with_code_highlighting(palette, cwd, width, true)
    }

    pub(super) fn new_for_metrics(
        palette: TerminalPalette,
        cwd: Option<&Path>,
        width: usize,
    ) -> Self {
        Self::new_with_code_highlighting(palette, cwd, width, false)
    }

    fn new_with_code_highlighting(
        palette: TerminalPalette,
        cwd: Option<&Path>,
        width: usize,
        should_highlight_code: bool,
    ) -> Self {
        Self {
            palette,
            cwd: cwd.map(Path::to_path_buf),
            width: width.max(1),
            should_highlight_code,
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

    pub(super) fn render<'a>(&mut self, parser: Parser<'a>) {
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

    pub(super) fn finish(mut self, width: usize) -> Vec<Line<'static>> {
        self.output
            .drain(..)
            .flat_map(|line| wrap_logical_line(line, width.max(1)))
            .collect()
    }

    pub(super) fn finish_metrics(mut self, width: usize) -> (usize, usize) {
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
            let highlighted = self
                .should_highlight_code
                .then(|| {
                    highlight_code_chunks(&code, &lang, self.highlighted_code_style()).map(
                        |lines| {
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
                        },
                    )
                })
                .flatten();

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
                self.quote_style().add_modifier(Modifier::BOLD),
            );
            push_chunk(
                &mut continuation,
                String::from("> "),
                self.quote_style().add_modifier(Modifier::BOLD),
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
        if self.blockquote_depth > 0 {
            return self.quote_style();
        }

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

    fn quote_style(&self) -> Style {
        quote_text_style(self.palette)
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
