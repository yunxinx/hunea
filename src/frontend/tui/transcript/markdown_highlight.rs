use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, Style as SyntectStyle, Theme, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::LinesWithEndings,
};

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME_SET: OnceLock<ThemeSet> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HighlightChunk {
    pub text: String,
    pub style: Style,
}

pub(super) fn highlight_code_chunks(
    code: &str,
    lang: &str,
    base_style: Style,
) -> Option<Vec<Vec<HighlightChunk>>> {
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
