use std::{borrow::Cow, collections::VecDeque, sync::OnceLock};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Style as SyntectStyle, Theme},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};
use two_face::theme::EmbeddedThemeName;
use unicode_segmentation::UnicodeSegmentation;

use super::wrap::{
    WrapSegmentKind, measure_width, should_start_new_wrap_segment, split_text_to_width,
    wrap_segment_kind,
};
use crate::{
    display_width::grapheme_width,
    theme::{TerminalColorCapability, TerminalPalette},
};

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

static HIGHLIGHT_ASSETS: OnceLock<MarkdownHighlightAssets> = OnceLock::new();

struct MarkdownHighlightAssets {
    syntax_set: SyntaxSet,
    dark_theme: Theme,
    light_theme: Theme,
}

impl MarkdownHighlightAssets {
    fn load() -> Self {
        let themes = two_face::theme::extra();
        Self {
            syntax_set: two_face::syntax::extra_newlines(),
            dark_theme: themes.get(EmbeddedThemeName::Base16OceanDark).clone(),
            light_theme: themes.get(EmbeddedThemeName::Base16OceanLight).clone(),
        }
    }

    fn theme(&self, palette: TerminalPalette) -> &Theme {
        if palette.color_capability() == TerminalColorCapability::ExplicitRgb
            && !palette.has_dark_background()
        {
            &self.light_theme
        } else {
            &self.dark_theme
        }
    }
}

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
    palette: TerminalPalette,
) -> Option<Vec<Vec<HighlightChunk>>> {
    #[cfg(test)]
    HIGHLIGHT_CODE_CHUNKS_CALL_COUNT.with(|count| count.set(count.get() + 1));

    if code.is_empty()
        || code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
    {
        return None;
    }

    let assets = highlight_assets();
    let syntax = find_syntax(&assets.syntax_set, lang)?;
    let theme = assets.theme(palette);
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(line, &assets.syntax_set).ok()?;
        let mut chunks = Vec::new();

        for (style, text) in ranges {
            let text = text.trim_end_matches(['\r', '\n']);
            if text.is_empty() {
                continue;
            }
            chunks.push(HighlightChunk {
                text: text.to_string(),
                style: base_style.patch(convert_syntect_style(style, palette)),
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
        let cluster_width = grapheme_width(grapheme);
        if *current_width > 0 && *current_width + cluster_width > width {
            lines.push(std::mem::take(current_spans));
            *current_width = 0;
        }

        push_highlight_span(current_spans, grapheme, style);
        *current_width += cluster_width;
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

fn highlight_assets() -> &'static MarkdownHighlightAssets {
    HIGHLIGHT_ASSETS.get_or_init(MarkdownHighlightAssets::load)
}

/// 在 UI 首次渲染代码块前初始化共享 syntax 与 theme 资产。
pub(crate) fn prewarm_markdown_highlighting() {
    let _ = highlight_assets();
}

fn find_syntax<'a>(syntax_set: &'a SyntaxSet, lang: &str) -> Option<&'a SyntaxReference> {
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

fn convert_syntect_style(style: SyntectStyle, palette: TerminalPalette) -> Style {
    let mut converted = Style::new();
    if palette.color_capability() == TerminalColorCapability::ExplicitRgb {
        converted = converted.fg(Color::Rgb(
            style.foreground.r,
            style.foreground.g,
            style.foreground.b,
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_highlighting_prewarm_is_idempotent_and_reuses_render_assets() {
        prewarm_markdown_highlighting();
        let first = highlight_assets() as *const MarkdownHighlightAssets;

        let highlighted = highlight_code_chunks(
            "fn main() {}\n",
            "rust",
            Style::new(),
            crate::theme::default_palette(),
        );
        let second = highlight_assets() as *const MarkdownHighlightAssets;

        assert!(highlighted.is_some());
        assert_eq!(first, second);
    }

    #[test]
    fn explicit_palettes_select_distinct_dark_and_light_syntax_themes() {
        let code = "pub fn main() { let answer = true; }\n";
        let dark = highlighted_foregrounds(
            code,
            crate::theme::palette_from_background(true, Some(Color::Rgb(18, 24, 32))),
        );
        let light = highlighted_foregrounds(
            code,
            crate::theme::palette_from_background(false, Some(Color::Rgb(242, 242, 242))),
        );

        assert!(!dark.is_empty());
        assert!(!light.is_empty());
        assert_ne!(dark, light);
    }

    #[test]
    fn terminal_default_conversion_keeps_font_modifiers_without_syntect_colors() {
        let converted = convert_syntect_style(
            SyntectStyle {
                foreground: syntect::highlighting::Color {
                    r: 10,
                    g: 20,
                    b: 30,
                    a: 255,
                },
                background: syntect::highlighting::Color {
                    r: 40,
                    g: 50,
                    b: 60,
                    a: 255,
                },
                font_style: FontStyle::BOLD | FontStyle::ITALIC | FontStyle::UNDERLINE,
            },
            crate::theme::terminal_default_palette(),
        );

        assert_eq!(converted.fg, None);
        assert_eq!(converted.bg, None);
        assert!(converted.add_modifier.contains(Modifier::BOLD));
        assert!(converted.add_modifier.contains(Modifier::ITALIC));
        assert!(converted.add_modifier.contains(Modifier::UNDERLINED));
    }

    fn highlighted_foregrounds(code: &str, palette: TerminalPalette) -> Vec<Color> {
        highlight_code_chunks(code, "rust", Style::new(), palette)
            .expect("Rust syntax should be available")
            .into_iter()
            .flatten()
            .filter_map(|chunk| chunk.style.fg)
            .collect()
    }
}
