use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

pub(super) use crate::display_width::display_width as measure_width;
use crate::display_width::grapheme_width;
use crate::transcript::linebreak::{
    ProseWrapOptions, WrappedWhitespace, flatten_styled_text, project_wrapped_styles,
    wrap_prose_ranges,
};

const DISPLAY_TAB_WIDTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WrapMode {
    Prose,
    Literal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StyledChunk {
    pub(super) text: String,
    pub(super) style: Style,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LogicalLine {
    pub(super) first_prefix: Vec<StyledChunk>,
    pub(super) continuation_prefix: Vec<StyledChunk>,
    pub(super) chunks: Vec<StyledChunk>,
    pub(super) wrap_mode: WrapMode,
    pub(super) preserve_trailing_spaces: bool,
}

#[derive(Debug, Clone)]
pub(super) struct OpenBlock {
    pub(super) first_prefix: Vec<StyledChunk>,
    pub(super) continuation_prefix: Vec<StyledChunk>,
    lines: Vec<Vec<StyledChunk>>,
    pub(super) wrap_mode: WrapMode,
    pub(super) preserve_trailing_spaces: bool,
}

impl OpenBlock {
    pub(super) fn new(
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

    pub(super) fn append_text(&mut self, text: &str, style: Style) {
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

    pub(super) fn append_text_without_newlines(&mut self, text: &str, style: Style) {
        let mut column = self.prefix_width_for_current_line() + chunk_width(self.current_line());
        for grapheme in UnicodeSegmentation::graphemes(text, true) {
            if grapheme == "\t" {
                let tab_width = tab_stop_width(column);
                push_chunk(self.current_line_mut(), " ".repeat(tab_width), style);
                column += tab_width;
                continue;
            }

            push_chunk(self.current_line_mut(), grapheme.to_string(), style);
            column += grapheme_width(grapheme);
        }
    }

    pub(super) fn append_styled_lines(&mut self, lines: Vec<Vec<StyledChunk>>) {
        for (index, line) in lines.into_iter().enumerate() {
            if index > 0 || chunk_width(self.current_line()) > 0 {
                self.newline();
            }
            self.current_line_mut().extend(line);
        }
    }

    pub(super) fn newline(&mut self) {
        self.lines.push(Vec::new());
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| chunk_width(line) == 0)
    }

    pub(super) fn into_logical_lines(mut self) -> Vec<LogicalLine> {
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

pub(super) fn wrap_logical_line(line: LogicalLine, width: usize) -> Vec<Line<'static>> {
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

pub(super) fn wrap_styled_chunks_for_width(
    chunks: &[StyledChunk],
    width: usize,
) -> Vec<Vec<StyledChunk>> {
    wrap_prose_chunks(chunks, width.max(1), width.max(1))
}

pub(super) fn measure_wrapped_logical_line(line: LogicalLine, width: usize) -> (usize, usize) {
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

fn wrap_prose_chunks(
    chunks: &[StyledChunk],
    first_width: usize,
    continuation_width: usize,
) -> Vec<Vec<StyledChunk>> {
    let (flat, style_ranges) = flatten_styled_text(
        chunks
            .iter()
            .map(|chunk| (chunk.text.as_str(), chunk.style)),
    );
    if flat.is_empty() {
        return vec![Vec::new()];
    }

    let wrapped = wrap_prose_ranges(
        &flat,
        ProseWrapOptions {
            first_width,
            continuation_width,
            wrapped_whitespace: WrappedWhitespace::Discard,
            trim_trailing_whitespace: true,
        },
    );
    project_wrapped_styles(&flat, &style_ranges, &wrapped)
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|styled| StyledChunk {
                    text: flat[styled.range].to_string(),
                    style: styled.style,
                })
                .collect()
        })
        .collect()
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

pub(super) fn push_chunk(chunks: &mut Vec<StyledChunk>, text: impl Into<String>, style: Style) {
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

pub(super) fn chunk_width(chunks: &[StyledChunk]) -> usize {
    chunks.iter().map(|chunk| measure_width(&chunk.text)).sum()
}

fn chunk_text_len(chunks: &[StyledChunk]) -> usize {
    chunks.iter().map(|chunk| chunk.text.len()).sum()
}

pub(super) fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn trim_display_math_text(text: &str) -> &str {
    text.trim_matches(['\n', '\r'])
}
