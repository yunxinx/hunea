use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    envinfo::short_work_dir,
    frontend::tui::{
        theme::{TerminalPalette, detect_palette},
        transcript::wrap_prompt_text,
    },
};

const DEFAULT_APP_NAME: &str = "Lumos";
const DEFAULT_VERSION: &str = "v0.1.0";
const BORDER_WIDTH: u16 = 2;
const HORIZONTAL_PADDING: u16 = 2;

/// `HeroOptions` 控制启动 hero 的文案和宽度。
/// `width` 为 0 时使用内容自然宽度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeroOptions {
    pub app_name: Option<String>,
    pub version: Option<String>,
    pub work_dir: Option<String>,
    pub width: u16,
}

/// `render_hero` 使用当前终端主题把启动 hero 渲染为 ANSI 字符串。
pub fn render_hero(options: &HeroOptions) -> String {
    render_hero_with_palette(options, detect_palette())
}

/// `render_hero_with_palette` 在给定语义配色下渲染启动 hero。
pub fn render_hero_with_palette(options: &HeroOptions, palette: TerminalPalette) -> String {
    let buffer = render_hero_buffer_with_palette(options, palette);
    buffer_to_ansi_string(&buffer)
}

/// `render_hero_buffer_with_palette` 直接返回 `Buffer`，便于测试布局和颜色语义。
pub fn render_hero_buffer_with_palette(options: &HeroOptions, palette: TerminalPalette) -> Buffer {
    let work_dir = options.work_dir.clone().unwrap_or_else(short_work_dir);
    render_hero_buffer(options, palette, &work_dir)
}

/// `render_hero_lines_with_palette` 将 hero 渲染为带样式的文本行，便于嵌入 transcript。
pub fn render_hero_lines_with_palette(
    options: &HeroOptions,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    buffer_to_lines(&render_hero_buffer_with_palette(options, palette))
}

/// `render_hero_plain_lines_with_palette` 返回不含 ANSI 的 hero 文本行。
pub fn render_hero_plain_lines_with_palette(
    options: &HeroOptions,
    palette: TerminalPalette,
) -> Vec<String> {
    buffer_to_plain_lines(&render_hero_buffer_with_palette(options, palette))
}

pub(crate) fn hero_total_width(options: &HeroOptions) -> u16 {
    let work_dir = options.work_dir.clone().unwrap_or_else(short_work_dir);
    let app_name = options.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let title_text = hero_title_plain_text(app_name, version);
    let content_width = resolved_content_width(options.width, &title_text, &work_dir);

    content_width + BORDER_WIDTH + (HORIZONTAL_PADDING * 2)
}

/// `print_hero` 直接把启动 hero 输出到标准输出。
pub fn print_hero(options: &HeroOptions) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    write_hero_to(&mut handle, options)
}

/// `write_hero_to` 把启动 hero 输出到任意 writer，并在结尾补换行。
pub fn write_hero_to<W: Write>(writer: &mut W, options: &HeroOptions) -> io::Result<()> {
    writeln!(writer, "{}", render_hero(options))
}

fn render_hero_buffer(options: &HeroOptions, palette: TerminalPalette, work_dir: &str) -> Buffer {
    let app_name = options.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let title_text = hero_title_plain_text(app_name, version);
    let content_width = resolved_content_width(options.width, &title_text, work_dir);
    let title_lines = wrap_prompt_text(&title_text, content_width as usize, 0);
    let work_dir_lines = if work_dir.is_empty() {
        Vec::new()
    } else {
        wrap_prompt_text(work_dir, content_width as usize, 0)
    };
    let total_width = content_width + BORDER_WIDTH + (HORIZONTAL_PADDING * 2);
    let content_height = title_lines.len() as u16
        + if work_dir_lines.is_empty() {
            0
        } else {
            1 + work_dir_lines.len() as u16
        };
    let total_height = content_height + BORDER_WIDTH;
    let mut buffer = Buffer::empty(ratatui::layout::Rect::new(0, 0, total_width, total_height));

    render_border_row(
        &mut buffer,
        0,
        0,
        total_width,
        BorderGlyphs {
            left: '╭',
            horizontal: '─',
            right: '╮',
        },
        palette.secondary,
    );
    render_border_row(
        &mut buffer,
        0,
        total_height.saturating_sub(1),
        total_width,
        BorderGlyphs {
            left: '╰',
            horizontal: '─',
            right: '╯',
        },
        palette.secondary,
    );

    for row in 1..total_height.saturating_sub(1) {
        set_cell(&mut buffer, 0, row, '│', palette.secondary, None);
        set_cell(
            &mut buffer,
            total_width.saturating_sub(1),
            row,
            '│',
            palette.secondary,
            None,
        );
    }

    let mut row = 1;
    let title_glyphs = hero_title_glyphs(app_name, version, palette);
    let mut title_offset = 0;
    for line in &title_lines {
        let glyph_count = line.chars().count();
        render_glyph_line(
            &mut buffer,
            row,
            &title_glyphs[title_offset..title_offset + glyph_count],
        );
        title_offset += glyph_count;
        row += 1;
    }

    if !work_dir_lines.is_empty() {
        row += 1;
        let work_dir_glyphs = monochrome_glyphs(work_dir, palette.secondary);
        let mut work_dir_offset = 0;
        for line in &work_dir_lines {
            let glyph_count = line.chars().count();
            render_glyph_line(
                &mut buffer,
                row,
                &work_dir_glyphs[work_dir_offset..work_dir_offset + glyph_count],
            );
            work_dir_offset += glyph_count;
            row += 1;
        }
    }

    buffer
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeroGlyph {
    character: char,
    foreground: Color,
}

pub(crate) fn hero_title_plain_text(app_name: &str, version: &str) -> String {
    format!(">_ {app_name} ({version})")
}

pub(crate) fn resolved_content_width(
    requested_width: u16,
    title_text: &str,
    work_dir: &str,
) -> u16 {
    if requested_width > 0 {
        return requested_width;
    }

    title_text.chars().count().max(work_dir.chars().count()) as u16
}

fn hero_title_glyphs(app_name: &str, version: &str, palette: TerminalPalette) -> Vec<HeroGlyph> {
    let mut glyphs = monochrome_glyphs(">_", palette.secondary);
    glyphs.extend(reset_glyphs(" "));
    glyphs.extend(monochrome_glyphs(app_name, palette.main));
    glyphs.extend(reset_glyphs(" "));
    glyphs.extend(monochrome_glyphs(
        &format!("({version})"),
        palette.secondary,
    ));
    glyphs
}

fn monochrome_glyphs(text: &str, color: Color) -> Vec<HeroGlyph> {
    text.chars()
        .map(|character| HeroGlyph {
            character,
            foreground: color,
        })
        .collect()
}

fn reset_glyphs(text: &str) -> Vec<HeroGlyph> {
    monochrome_glyphs(text, Color::Reset)
}

fn render_glyph_line(buffer: &mut Buffer, y: u16, glyphs: &[HeroGlyph]) {
    let mut cursor_x = 1 + HORIZONTAL_PADDING;
    for glyph in glyphs {
        set_cell(buffer, cursor_x, y, glyph.character, glyph.foreground, None);
        cursor_x += 1;
    }
}

struct BorderGlyphs {
    left: char,
    horizontal: char,
    right: char,
}

fn render_border_row(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    glyphs: BorderGlyphs,
    color: Color,
) {
    set_cell(buffer, x, y, glyphs.left, color, None);
    for column in x + 1..x + width - 1 {
        set_cell(buffer, column, y, glyphs.horizontal, color, None);
    }
    set_cell(buffer, x + width - 1, y, glyphs.right, color, None);
}

fn set_cell(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    character: char,
    foreground: Color,
    background: Option<Color>,
) {
    let cell = &mut buffer[(x, y)];
    cell.set_char(character);
    cell.set_fg(foreground);
    if let Some(background) = background {
        cell.set_bg(background);
    }
}

fn buffer_to_lines(buffer: &Buffer) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut spans = Vec::new();
        let mut current_style = Style::new();
        let mut current_text = String::new();
        let mut is_first_cell = true;

        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            let cell_style = cell.style();

            if is_first_cell {
                current_style = cell_style;
                is_first_cell = false;
            }

            if cell_style != current_style {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
                current_style = cell_style;
            }

            current_text.push_str(cell.symbol());
        }

        spans.push(Span::styled(current_text, current_style));
        lines.push(Line::default().spans(spans));
    }

    lines
}

fn buffer_to_plain_lines(buffer: &Buffer) -> Vec<String> {
    let mut lines = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut line = String::new();
        for column in 0..buffer.area.width {
            line.push_str(buffer[(column, row)].symbol());
        }
        lines.push(line);
    }

    lines
}

fn buffer_to_ansi_string(buffer: &Buffer) -> String {
    let mut rendered = String::new();

    for row in 0..buffer.area.height {
        let mut active_style = Style::new();

        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            let style = cell.style();
            if style != active_style {
                push_style_escape(&mut rendered, style);
                active_style = style;
            }
            rendered.push_str(cell.symbol());
        }

        if active_style != Style::new() {
            rendered.push_str("\u{1b}[0m");
        }

        if row + 1 < buffer.area.height {
            rendered.push('\n');
        }
    }

    rendered
}

fn push_style_escape(rendered: &mut String, style: Style) {
    let mut codes = Vec::new();

    match style.fg {
        Some(Color::Reset) | None => {}
        Some(color) => codes.push(foreground_code(color)),
    }

    match style.bg {
        Some(Color::Reset) | None => {}
        Some(color) => codes.push(background_code(color)),
    }

    if style.add_modifier.contains(ratatui::style::Modifier::BOLD) {
        codes.push(String::from("1"));
    }

    if codes.is_empty() {
        rendered.push_str("\u{1b}[0m");
        return;
    }

    rendered.push_str("\u{1b}[");
    rendered.push_str(&codes.join(";"));
    rendered.push('m');
}

fn foreground_code(color: Color) -> String {
    match color {
        Color::Black => String::from("30"),
        Color::Red => String::from("31"),
        Color::Green => String::from("32"),
        Color::Yellow => String::from("33"),
        Color::Blue => String::from("34"),
        Color::Magenta => String::from("35"),
        Color::Cyan => String::from("36"),
        Color::Gray => String::from("37"),
        Color::DarkGray => String::from("90"),
        Color::LightRed => String::from("91"),
        Color::LightGreen => String::from("92"),
        Color::LightYellow => String::from("93"),
        Color::LightBlue => String::from("94"),
        Color::LightMagenta => String::from("95"),
        Color::LightCyan => String::from("96"),
        Color::White => String::from("97"),
        Color::Indexed(index) => format!("38;5;{index}"),
        Color::Rgb(red, green, blue) => format!("38;2;{red};{green};{blue}"),
        Color::Reset => String::from("39"),
    }
}

fn background_code(color: Color) -> String {
    match color {
        Color::Black => String::from("40"),
        Color::Red => String::from("41"),
        Color::Green => String::from("42"),
        Color::Yellow => String::from("43"),
        Color::Blue => String::from("44"),
        Color::Magenta => String::from("45"),
        Color::Cyan => String::from("46"),
        Color::Gray => String::from("47"),
        Color::DarkGray => String::from("100"),
        Color::LightRed => String::from("101"),
        Color::LightGreen => String::from("102"),
        Color::LightYellow => String::from("103"),
        Color::LightBlue => String::from("104"),
        Color::LightMagenta => String::from("105"),
        Color::LightCyan => String::from("106"),
        Color::White => String::from("107"),
        Color::Indexed(index) => format!("48;5;{index}"),
        Color::Rgb(red, green, blue) => format!("48;2;{red};{green};{blue}"),
        Color::Reset => String::from("49"),
    }
}

#[cfg(test)]
mod tests {
    use super::{HeroOptions, render_hero_buffer};
    use crate::frontend::tui::theme::palette_from_background;

    #[test]
    fn render_keeps_title_width_when_work_dir_is_absent() {
        let buffer = render_hero_buffer(&HeroOptions::default(), sample_palette(), "");

        assert_eq!(buffer.area.width, 23);
        assert_eq!(buffer.area.height, 3);
    }

    fn sample_palette() -> crate::frontend::tui::theme::TerminalPalette {
        palette_from_background(true, None)
    }
}
