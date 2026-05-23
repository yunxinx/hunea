use std::{borrow::Cow, collections::VecDeque, sync::OnceLock};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Style as SyntectStyle, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::wrap::{
    WrapSegmentKind, measure_width, should_start_new_wrap_segment, split_text_to_width,
    wrap_segment_kind,
};

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static HIGHLIGHT_CODE_CHUNKS_CALL_COUNT: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HighlightChunk {
    pub(crate) text: String,
    pub(crate) style: Style,
}

#[derive(Debug, Clone)]
struct HighlightSegment {
    text: String,
    style: Style,
    width: usize,
    is_space: bool,
}

pub(crate) fn highlight_code_chunks(
    code: &str,
    lang: &str,
    base_style: Style,
) -> Option<Vec<Vec<HighlightChunk>>> {
    #[cfg(test)]
    HIGHLIGHT_CODE_CHUNKS_CALL_COUNT.with(|count| count.set(count.get() + 1));

    if code.is_empty()
        || code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
    {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let theme = default_theme()?;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
        let mut chunks = Vec::new();

        for (style, text) in ranges {
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                continue;
            }
            chunks.push(HighlightChunk {
                text: text.to_string(),
                style: base_style.patch(convert_syntect_style(style)),
            });
        }

        if chunks.is_empty() {
            chunks.push(HighlightChunk {
                text: String::new(),
                style: base_style,
            });
        }
        lines.push(chunks);
    }

    Some(lines)
}

#[cfg(test)]
pub(crate) fn reset_highlight_code_chunks_call_count() {
    HIGHLIGHT_CODE_CHUNKS_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn highlight_code_chunks_call_count() -> usize {
    HIGHLIGHT_CODE_CHUNKS_CALL_COUNT.with(std::cell::Cell::get)
}

/// `wrap_highlight_chunks` 按终端宽度折行已高亮的文本片段。
pub(crate) fn wrap_highlight_chunks(
    highlighted_lines: &[Vec<HighlightChunk>],
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for highlighted_line in highlighted_lines {
        let mut current_spans = Vec::new();
        let mut current_width = 0usize;

        for chunk in highlighted_line {
            append_wrapped_highlight_chunk(
                &mut lines,
                &mut current_spans,
                &mut current_width,
                &chunk.text,
                chunk.style,
                width,
            );
        }

        lines.push(current_spans);
    }

    lines
}

/// `wrap_highlight_chunks_soft` 保留高亮样式，同时按 prose soft-wrap 规则折行。
pub(crate) fn wrap_highlight_chunks_soft(
    highlighted_lines: &[Vec<HighlightChunk>],
    width: usize,
) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for highlighted_line in highlighted_lines {
        let segments = tokenize_highlight_segments(highlighted_line);
        if segments.is_empty() {
            lines.push(Vec::new());
            continue;
        }

        let mut cursor = VecDeque::from(segments);
        while !cursor.is_empty() {
            lines.push(consume_soft_highlight_line(&mut cursor, width));
        }
    }

    lines
}

fn append_wrapped_highlight_chunk(
    lines: &mut Vec<Vec<Span<'static>>>,
    current_spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    text: &str,
    style: Style,
    width: usize,
) {
    for grapheme in UnicodeSegmentation::graphemes(text, true) {
        let grapheme_width = grapheme.width();
        if *current_width > 0 && *current_width + grapheme_width > width {
            lines.push(std::mem::take(current_spans));
            *current_width = 0;
        }

        push_highlight_span(current_spans, grapheme, style);
        *current_width += grapheme_width;
    }
}

fn consume_soft_highlight_line(
    cursor: &mut VecDeque<HighlightSegment>,
    width: usize,
) -> Vec<Span<'static>> {
    let mut line = Vec::new();
    let mut line_width = 0usize;
    let mut pending_spaces = Vec::new();
    let mut pending_space_width = 0usize;

    while let Some(segment) = cursor.pop_front() {
        if segment.is_space {
            if line_width == 0 {
                if segment.width <= width {
                    push_highlight_span(&mut line, &segment.text, segment.style);
                    line_width = line_width.saturating_add(segment.width);
                } else {
                    let (fitted, overflow) = split_highlight_segment_to_width(segment, width);
                    push_highlight_span(&mut line, &fitted.text, fitted.style);
                    if overflow.width > 0 {
                        cursor.push_front(overflow);
                    }
                }
                continue;
            }

            pending_space_width = pending_space_width.saturating_add(segment.width);
            pending_spaces.push(segment);
            continue;
        }

        if line_width == 0 {
            if segment.width <= width {
                push_highlight_span(&mut line, &segment.text, segment.style);
                line_width = line_width.saturating_add(segment.width);
            } else {
                let (fitted, overflow) = split_highlight_segment_to_width(segment, width);
                push_highlight_span(&mut line, &fitted.text, fitted.style);
                if overflow.width > 0 {
                    cursor.push_front(overflow);
                }
                break;
            }
            continue;
        }

        if line_width + pending_space_width + segment.width <= width {
            for space in pending_spaces.drain(..) {
                push_highlight_span(&mut line, &space.text, space.style);
            }
            push_highlight_span(&mut line, &segment.text, segment.style);
            line_width = line_width
                .saturating_add(pending_space_width)
                .saturating_add(segment.width);
            pending_space_width = 0;
            continue;
        }

        cursor.push_front(segment);
        break;
    }

    trim_trailing_highlight_spaces(&mut line);
    line
}

fn tokenize_highlight_segments(chunks: &[HighlightChunk]) -> Vec<HighlightSegment> {
    let mut segments = Vec::new();

    for chunk in chunks {
        let mut current = String::new();
        let mut current_width = 0usize;
        let mut current_kind = None;

        for grapheme in UnicodeSegmentation::graphemes(chunk.text.as_str(), true) {
            let kind = wrap_segment_kind(grapheme);
            match current_kind {
                Some(existing) if should_start_new_wrap_segment(existing, kind) => {
                    segments.push(HighlightSegment {
                        text: std::mem::take(&mut current),
                        style: chunk.style,
                        width: current_width,
                        is_space: existing == WrapSegmentKind::Space,
                    });
                    current_width = 0;
                    current_kind = Some(kind);
                }
                None => current_kind = Some(kind),
                _ => {}
            }

            current.push_str(grapheme);
            current_width = current_width.saturating_add(measure_width(grapheme));
        }

        if let Some(kind) = current_kind {
            segments.push(HighlightSegment {
                text: current,
                style: chunk.style,
                width: current_width,
                is_space: kind == WrapSegmentKind::Space,
            });
        }
    }

    segments
}

fn split_highlight_segment_to_width(
    segment: HighlightSegment,
    width: usize,
) -> (HighlightSegment, HighlightSegment) {
    let (fitted_text, overflow_text) = split_text_to_width(&segment.text, width);

    (
        HighlightSegment {
            width: measure_width(&fitted_text),
            text: fitted_text,
            style: segment.style,
            is_space: segment.is_space,
        },
        HighlightSegment {
            width: measure_width(&overflow_text),
            text: overflow_text,
            style: segment.style,
            is_space: segment.is_space,
        },
    )
}

fn push_highlight_span(spans: &mut Vec<Span<'static>>, text: &str, style: Style) {
    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        let mut content = last.content.to_string();
        content.push_str(text);
        last.content = Cow::Owned(content);
        return;
    }

    spans.push(Span::styled(text.to_string(), style));
}

fn trim_trailing_highlight_spaces(spans: &mut Vec<Span<'static>>) {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.content.trim_end_matches(char::is_whitespace);
        if trimmed.len() == last.content.len() {
            break;
        }

        if trimmed.is_empty() {
            spans.pop();
            continue;
        }

        last.content = Cow::Owned(trimmed.to_string());
        break;
    }
}

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme_set() -> &'static ThemeSet {
    THEME_SET.get_or_init(|| two_face::theme::extra().into())
}

fn default_theme() -> Option<&'static Theme> {
    let themes = &theme_set().themes;
    themes
        .get("base16-ocean.dark")
        .or_else(|| themes.get("InspiredGitHub"))
        .or_else(|| themes.values().next())
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let syntax_set = syntax_set();
    let lang = lang.trim();
    if lang.is_empty() {
        return None;
    }

    let lang = match lang {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" | "sh" => "bash",
        other => other,
    };

    syntax_set
        .find_syntax_by_token(lang)
        .or_else(|| syntax_set.find_syntax_by_extension(lang))
        .or_else(|| syntax_set.find_syntax_by_name(lang))
        .or_else(|| {
            let lower = lang.to_ascii_lowercase();
            syntax_set
                .syntaxes()
                .iter()
                .find(|syntax| syntax.name.to_ascii_lowercase() == lower)
        })
}

fn convert_syntect_style(style: SyntectStyle) -> Style {
    let mut converted = Style::new().fg(Color::Rgb(
        style.foreground.r,
        style.foreground.g,
        style.foreground.b,
    ));

    if style.font_style.contains(FontStyle::BOLD) {
        converted = converted.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        converted = converted.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        converted = converted.add_modifier(Modifier::UNDERLINED);
    }

    converted
}
