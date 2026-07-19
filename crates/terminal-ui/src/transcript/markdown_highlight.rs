use std::{borrow::Cow, sync::OnceLock};

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

use super::linebreak::{
    ProseWrapOptions, WrappedWhitespace, flatten_styled_text, project_wrapped_styles,
    wrap_prose_ranges,
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
        let (flat, style_ranges) = flatten_styled_text(
            highlighted_line
                .iter()
                .map(|chunk| (chunk.text.as_str(), chunk.style)),
        );
        if flat.is_empty() {
            lines.push(Vec::new());
            continue;
        }

        let wrapped = wrap_prose_ranges(
            &flat,
            ProseWrapOptions {
                first_width: width,
                continuation_width: width,
                wrapped_whitespace: WrappedWhitespace::Discard,
                trim_trailing_whitespace: true,
            },
        );
        for projected_line in project_wrapped_styles(&flat, &style_ranges, &wrapped) {
            let mut spans = Vec::new();
            for styled in projected_line {
                push_highlight_span(&mut spans, &flat[styled.range], styled.style);
            }
            lines.push(spans);
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

    #[test]
    fn soft_wrap_does_not_treat_highlight_style_change_as_a_breakpoint() {
        let highlighted = vec![vec![
            HighlightChunk {
                text: "你".to_string(),
                style: Style::new().fg(Color::Red),
            },
            HighlightChunk {
                text: "，好".to_string(),
                style: Style::new().fg(Color::Blue),
            },
        ]];

        let lines = wrap_highlight_chunks_soft(&highlighted, 4);
        let plain = lines
            .iter()
            .map(|line| {
                line.iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert_eq!(plain, vec!["你，", "好"]);
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
