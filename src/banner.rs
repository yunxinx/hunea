use std::io::{self, Write};

use ratatui::{buffer::Buffer, layout::Rect, style::Color, widgets::Widget};

use crate::theme::{TerminalPalette, detect_palette};

const DEFAULT_APP_NAME: &str = "Lumos";
const DEFAULT_VERSION: &str = "v0.0.1";
const BORDER_WIDTH: u16 = 2;
const BANNER_HEIGHT: u16 = 3;
const HORIZONTAL_PADDING: u16 = 2;

/// RenderOptions 控制启动 banner 的文案和宽度。
/// `width` 表示 banner 内容区的最小宽度；为 0 时使用内容自然宽度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderOptions {
    pub app_name: Option<String>,
    pub version: Option<String>,
    pub width: u16,
}

/// Render 使用当前终端主题把 banner 渲染为 ANSI 字符串。
pub fn render(options: &RenderOptions) -> String {
    render_with_palette(options, detect_palette())
}

/// RenderWithPalette 在给定语义配色下渲染 banner。
/// 这个入口让运行时探测和组件渲染保持解耦，后续接到完整 TUI 时也更容易复用。
pub fn render_with_palette(options: &RenderOptions, palette: TerminalPalette) -> String {
    let buffer = render_buffer_with_palette(options, palette);
    buffer_to_ansi_string(&buffer)
}

/// RenderBufferWithPalette 直接返回 Ratatui `Buffer`，便于在测试里验证布局与颜色语义。
pub fn render_buffer_with_palette(options: &RenderOptions, palette: TerminalPalette) -> Buffer {
    let app_name = options.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let content = banner_content(app_name, version);
    let content_width = options.width.max(content_width(&content));
    let total_width = content_width + BORDER_WIDTH + (HORIZONTAL_PADDING * 2);
    let area = Rect::new(0, 0, total_width, BANNER_HEIGHT);
    let mut buffer = Buffer::empty(area);

    StartupBanner {
        app_name,
        version,
        palette,
    }
    .render(area, &mut buffer);

    buffer
}

/// Print 直接把 banner 写到标准输出。
pub fn print(options: &RenderOptions) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    write_to(&mut handle, options)
}

/// WriteTo 把 banner 输出到任意 writer，并在结尾补换行，方便命令行入口直接复用。
pub fn write_to<W: Write>(writer: &mut W, options: &RenderOptions) -> io::Result<()> {
    writeln!(writer, "{}", render(options))
}

struct StartupBanner<'a> {
    app_name: &'a str,
    version: &'a str,
    palette: TerminalPalette,
}

impl Widget for StartupBanner<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < BORDER_WIDTH || area.height < BANNER_HEIGHT {
            return;
        }

        render_border_row(
            buf,
            area.x,
            area.y,
            area.width,
            BorderGlyphs {
                left: '╭',
                horizontal: '─',
                right: '╮',
            },
            self.palette.secondary,
        );
        render_border_row(
            buf,
            area.x,
            area.y + 2,
            area.width,
            BorderGlyphs {
                left: '╰',
                horizontal: '─',
                right: '╯',
            },
            self.palette.secondary,
        );

        let middle_y = area.y + 1;
        set_cell(buf, area.x, middle_y, '│', self.palette.secondary);
        set_cell(
            buf,
            area.x + area.width.saturating_sub(1),
            middle_y,
            '│',
            self.palette.secondary,
        );

        let mut cursor_x = area.x + 1 + HORIZONTAL_PADDING;
        write_styled_text(buf, &mut cursor_x, middle_y, ">_", self.palette.secondary);
        write_plain_text(buf, &mut cursor_x, middle_y, " ");
        write_styled_text(
            buf,
            &mut cursor_x,
            middle_y,
            self.app_name,
            self.palette.main,
        );
        write_plain_text(buf, &mut cursor_x, middle_y, " ");
        let version = format!("({})", self.version);
        write_styled_text(
            buf,
            &mut cursor_x,
            middle_y,
            &version,
            self.palette.secondary,
        );

        let content_end = area.x + area.width - 1 - HORIZONTAL_PADDING;
        while cursor_x < content_end {
            write_plain_text(buf, &mut cursor_x, middle_y, " ");
        }
    }
}

fn banner_content(app_name: &str, version: &str) -> String {
    format!(">_ {} ({})", app_name, version)
}

fn content_width(content: &str) -> u16 {
    content.chars().count() as u16
}

struct BorderGlyphs {
    left: char,
    horizontal: char,
    right: char,
}

fn render_border_row(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    glyphs: BorderGlyphs,
    color: Color,
) {
    set_cell(buf, x, y, glyphs.left, color);
    for column in x + 1..x + width - 1 {
        set_cell(buf, column, y, glyphs.horizontal, color);
    }
    set_cell(buf, x + width - 1, y, glyphs.right, color);
}

fn write_styled_text(buf: &mut Buffer, cursor_x: &mut u16, y: u16, text: &str, color: Color) {
    for character in text.chars() {
        set_cell(buf, *cursor_x, y, character, color);
        *cursor_x += 1;
    }
}

fn write_plain_text(buf: &mut Buffer, cursor_x: &mut u16, y: u16, text: &str) {
    for character in text.chars() {
        set_cell(buf, *cursor_x, y, character, Color::Reset);
        *cursor_x += 1;
    }
}

fn set_cell(buf: &mut Buffer, x: u16, y: u16, character: char, color: Color) {
    let cell = &mut buf[(x, y)];
    cell.set_char(character);
    cell.set_fg(color);
}

fn buffer_to_ansi_string(buffer: &Buffer) -> String {
    let mut rendered = String::new();

    for row in 0..buffer.area.height {
        let mut active_color = Color::Reset;

        for column in 0..buffer.area.width {
            let cell = &buffer[(column, row)];
            if cell.fg != active_color {
                push_foreground_escape(&mut rendered, cell.fg);
                active_color = cell.fg;
            }
            rendered.push_str(cell.symbol());
        }

        if active_color != Color::Reset {
            rendered.push_str("\u{1b}[39m");
        }

        if row + 1 < buffer.area.height {
            rendered.push('\n');
        }
    }

    rendered
}

fn push_foreground_escape(rendered: &mut String, color: Color) {
    match color {
        Color::Reset => rendered.push_str("\u{1b}[39m"),
        _ => {
            rendered.push_str("\u{1b}[");
            rendered.push_str(&foreground_code(color));
            rendered.push('m');
        }
    }
}

fn foreground_code(color: Color) -> String {
    match color {
        Color::Reset => String::from("39"),
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
    }
}
