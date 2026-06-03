//! 启动 banner 的渲染与终端输出。

mod entrance;
mod item;

use std::io::{self, Write};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use runtime_domain::envinfo::short_work_dir;

use crate::{
    display_width::{display_width, line_display_width},
    styled_text::{line_to_plain_text, lines_to_ansi_text},
    theme::{TerminalPalette, detect_palette},
    transcript::{display_tab_width, wrap_prompt_visual_lines},
};

pub(crate) use entrance::StartupBannerEntranceState;
pub(crate) use item::StartupBannerItem;

pub(super) const DEFAULT_APP_NAME: &str = "Hunea";
pub(super) const DEFAULT_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const BORDER_WIDTH: u16 = 2;
const HORIZONTAL_PADDING: u16 = 1;

/// `StartupBannerOptions` 控制启动欢迎块的文案和宽度。
/// `width` 为 0 时使用内容自然宽度。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupBannerOptions {
    pub app_name: Option<String>,
    pub version: Option<String>,
    pub model_name: Option<String>,
    pub work_dir: Option<String>,
    pub width: u16,
}

/// `render_startup_banner` 使用当前终端主题把启动欢迎块渲染为 ANSI 字符串。
pub fn render_startup_banner(options: &StartupBannerOptions) -> String {
    render_startup_banner_with_palette(options, detect_palette())
}

/// `render_startup_banner_with_palette` 在给定语义配色下渲染启动欢迎块。
pub fn render_startup_banner_with_palette(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
) -> String {
    lines_to_ansi_text(&render_startup_banner_lines_with_palette(options, palette))
}

/// `render_startup_banner_buffer_with_palette` 直接返回 `Buffer`，便于测试布局和颜色语义。
pub fn render_startup_banner_buffer_with_palette(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
) -> Buffer {
    let lines = render_startup_banner_lines_with_palette(options, palette);
    startup_banner_lines_to_buffer(&lines)
}

fn startup_banner_lines_to_buffer(lines: &[Line<'static>]) -> Buffer {
    let width = lines
        .iter()
        .map(line_display_width)
        .max()
        .unwrap_or_default();
    let width = to_u16_width(width);
    let height = to_u16_width(lines.len());
    let mut buffer = Buffer::empty(Rect::new(0, 0, width, height));

    for (row, line) in lines.iter().enumerate() {
        buffer.set_line(0, to_u16_width(row), line, width);
    }

    buffer
}

/// `render_startup_banner_lines_with_palette` 将启动欢迎块渲染为带样式的文本行，便于嵌入 transcript。
pub fn render_startup_banner_lines_with_palette(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
) -> Vec<Line<'static>> {
    let work_dir = options.work_dir.clone().unwrap_or_else(short_work_dir);
    render_startup_banner_lines(options, palette, &work_dir)
}

/// `render_startup_banner_plain_lines_with_palette` 返回不含 ANSI 的启动欢迎块文本行。
pub fn render_startup_banner_plain_lines_with_palette(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
) -> Vec<String> {
    render_startup_banner_lines_with_palette(options, palette)
        .iter()
        .map(line_to_plain_text)
        .collect()
}

pub(crate) fn startup_banner_total_width(options: &StartupBannerOptions) -> u16 {
    let work_dir = options.work_dir.clone().unwrap_or_else(short_work_dir);
    let app_name = options.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let plain_rows =
        startup_banner_plain_rows(app_name, version, options.model_name.as_deref(), &work_dir);
    let title = startup_banner_title_plain_text(app_name, version);
    if plain_rows.is_empty() {
        return to_u16_width(display_width(&title));
    }

    let content_width = resolved_startup_banner_content_width(options.width, &title, &plain_rows);

    content_width + BORDER_WIDTH + (HORIZONTAL_PADDING * 2)
}

/// `print_startup_banner` 直接把启动欢迎块输出到标准输出。
pub fn print_startup_banner(options: &StartupBannerOptions) -> io::Result<()> {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    write_startup_banner_to(&mut handle, options)
}

/// `write_startup_banner_to` 把启动欢迎块输出到任意 writer，并在结尾补换行。
pub fn write_startup_banner_to<W: Write>(
    writer: &mut W,
    options: &StartupBannerOptions,
) -> io::Result<()> {
    writeln!(writer, "{}", render_startup_banner(options))
}

fn render_startup_banner_lines(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
    work_dir: &str,
) -> Vec<Line<'static>> {
    let app_name = options.app_name.as_deref().unwrap_or(DEFAULT_APP_NAME);
    let version = options.version.as_deref().unwrap_or(DEFAULT_VERSION);
    let rows = startup_banner_rows(app_name, version, options.model_name.as_deref(), work_dir);

    if rows.is_empty() {
        return vec![startup_banner_title_row(app_name, version).to_line(palette)];
    }

    let plain_rows = rows
        .iter()
        .map(StartupBannerRow::plain_text)
        .collect::<Vec<_>>();
    let title = startup_banner_title_plain_text(app_name, version);
    let content_width = resolved_startup_banner_content_width(options.width, &title, &plain_rows);
    let content_width_usize = usize::from(content_width);
    let mut lines = Vec::new();

    lines.push(startup_banner_border_line(
        content_width_usize,
        '╭',
        '╮',
        palette,
    ));
    for row in rows {
        for wrapped_line in wrap_startup_banner_line(row.to_line(palette), content_width_usize) {
            lines.push(startup_banner_framed_line(
                wrapped_line,
                content_width_usize,
                palette,
            ));
        }
    }
    lines.push(startup_banner_border_line(
        content_width_usize,
        '╰',
        '╯',
        palette,
    ));

    lines
}

#[cfg(test)]
fn render_startup_banner_buffer(
    options: &StartupBannerOptions,
    palette: TerminalPalette,
    work_dir: &str,
) -> Buffer {
    startup_banner_lines_to_buffer(&render_startup_banner_lines(options, palette, work_dir))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StartupBannerRow {
    spans: Vec<StartupBannerTextSpan>,
}

impl StartupBannerRow {
    fn from_spans(spans: Vec<StartupBannerTextSpan>) -> Self {
        Self { spans }
    }

    fn to_line(&self, palette: TerminalPalette) -> Line<'static> {
        let mut spans = Vec::new();
        let mut current_width = 0usize;

        for span in &self.spans {
            append_text_as_styled_spans(
                &mut spans,
                &mut current_width,
                &span.text,
                startup_banner_style_for_role(span.role, palette),
            );
        }

        Line::from(spans)
    }

    fn plain_text(&self) -> String {
        let mut plain_text = String::new();
        let mut current_width = 0usize;

        for span in &self.spans {
            append_text_for_plain_row(&mut plain_text, &mut current_width, &span.text);
        }

        plain_text
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartupBannerTextSpan {
    text: String,
    role: StartupBannerStyleRole,
}

impl StartupBannerTextSpan {
    fn new(text: impl Into<String>, role: StartupBannerStyleRole) -> Self {
        Self {
            text: text.into(),
            role,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupBannerStyleRole {
    Main,
    Secondary,
    Tertiary,
    CommandAccent,
    Reset,
}

pub(crate) fn startup_banner_title_plain_text(app_name: &str, version: &str) -> String {
    startup_banner_title_row(app_name, version).plain_text()
}

pub(crate) fn resolved_startup_banner_content_width(
    requested_width: u16,
    title_segment: &str,
    rows: &[String],
) -> u16 {
    if requested_width > 0 {
        return requested_width;
    }

    rows.iter()
        .map(|row| display_width(row))
        .chain(std::iter::once(display_width(title_segment)))
        .max()
        .map(to_u16_width)
        .unwrap_or_default()
}

const MODEL_LABEL: &str = "model:";
const DIRECTORY_LABEL: &str = "directory:";
const MODEL_CHANGE_HINT_COMMAND: &str = "/models";
const MODEL_CHANGE_HINT_EXPLANATION: &str = " to change";

pub(crate) fn startup_banner_plain_rows(
    app_name: &str,
    version: &str,
    model_name: Option<&str>,
    work_dir: &str,
) -> Vec<String> {
    startup_banner_rows(app_name, version, model_name, work_dir)
        .iter()
        .map(StartupBannerRow::plain_text)
        .collect()
}

pub(crate) fn startup_banner_model_plain_text(model_name: &str) -> String {
    startup_banner_model_row(model_name).plain_text()
}

pub(crate) fn startup_banner_directory_plain_text(work_dir: &str) -> String {
    startup_banner_directory_row(work_dir).plain_text()
}

fn startup_banner_rows(
    app_name: &str,
    version: &str,
    model_name: Option<&str>,
    work_dir: &str,
) -> Vec<StartupBannerRow> {
    let model_name = model_name.filter(|model_name| !model_name.is_empty());
    let has_metadata = model_name.is_some() || !work_dir.is_empty();
    let mut rows = Vec::with_capacity(4);

    if !has_metadata {
        return rows;
    }

    rows.push(startup_banner_title_row(app_name, version));
    rows.push(StartupBannerRow::default());
    if let Some(model_name) = model_name {
        rows.push(startup_banner_model_row(model_name));
    }
    if !work_dir.is_empty() {
        rows.push(startup_banner_directory_row(work_dir));
    }

    rows
}

fn startup_banner_title_row(app_name: &str, version: &str) -> StartupBannerRow {
    StartupBannerRow::from_spans(vec![
        StartupBannerTextSpan::new(app_name, StartupBannerStyleRole::Main),
        StartupBannerTextSpan::new(" ", StartupBannerStyleRole::Reset),
        StartupBannerTextSpan::new(format!("({version})"), StartupBannerStyleRole::Secondary),
    ])
}

fn startup_banner_model_row(model_name: &str) -> StartupBannerRow {
    StartupBannerRow::from_spans(vec![
        StartupBannerTextSpan::new(
            startup_banner_model_prefix(),
            StartupBannerStyleRole::Tertiary,
        ),
        StartupBannerTextSpan::new(model_name, StartupBannerStyleRole::Main),
        StartupBannerTextSpan::new("   ", StartupBannerStyleRole::Reset),
        StartupBannerTextSpan::new(
            MODEL_CHANGE_HINT_COMMAND,
            StartupBannerStyleRole::CommandAccent,
        ),
        StartupBannerTextSpan::new(
            MODEL_CHANGE_HINT_EXPLANATION,
            StartupBannerStyleRole::Tertiary,
        ),
    ])
}

fn startup_banner_directory_row(work_dir: &str) -> StartupBannerRow {
    StartupBannerRow::from_spans(vec![
        StartupBannerTextSpan::new(
            startup_banner_directory_prefix(),
            StartupBannerStyleRole::Tertiary,
        ),
        StartupBannerTextSpan::new(work_dir, StartupBannerStyleRole::Main),
    ])
}

fn startup_banner_model_prefix() -> String {
    startup_banner_label_prefix(MODEL_LABEL)
}

fn startup_banner_directory_prefix() -> String {
    startup_banner_label_prefix(DIRECTORY_LABEL)
}

fn startup_banner_label_prefix(label: &str) -> String {
    format!("{label:<width$} ", width = DIRECTORY_LABEL.len())
}

fn wrap_startup_banner_line(line: Line<'static>, content_width: usize) -> Vec<Line<'static>> {
    let plain_text = line_to_plain_text(&line);
    if plain_text.is_empty() {
        return vec![Line::raw("")];
    }

    wrap_prompt_visual_lines(&plain_text, content_width.max(1), 0)
        .into_iter()
        .map(|wrapped_line| {
            slice_styled_line_by_char_range(
                &line,
                wrapped_line.visible_start_char,
                wrapped_line.end_char,
            )
        })
        .collect()
}

fn slice_styled_line_by_char_range(
    line: &Line<'static>,
    start_char: usize,
    end_char: usize,
) -> Line<'static> {
    let mut spans = Vec::new();
    let mut cursor = 0usize;

    for span in &line.spans {
        let mut selected_text = String::new();

        for cluster in UnicodeSegmentation::graphemes(span.content.as_ref(), true) {
            let cluster_start = cursor;
            let cluster_end = cursor + cluster.chars().count();
            if cluster_end > start_char && cluster_start < end_char {
                selected_text.push_str(cluster);
            }
            cursor = cluster_end;
            if cursor >= end_char {
                break;
            }
        }

        if !selected_text.is_empty() {
            push_styled_span(&mut spans, selected_text, span.style);
        }
        if cursor >= end_char {
            break;
        }
    }

    Line::from(spans)
}

fn startup_banner_framed_line(
    content_line: Line<'static>,
    content_width: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let padding_width = content_width.saturating_sub(line_display_width(&content_line));
    let mut spans = Vec::with_capacity(content_line.spans.len() + 5);
    let border_style = style_for_color(palette.secondary);
    spans.push(Span::styled("│", border_style));
    spans.push(Span::raw(" ".repeat(usize::from(HORIZONTAL_PADDING))));
    spans.extend(content_line.spans);
    spans.push(Span::raw(" ".repeat(padding_width)));
    spans.push(Span::raw(" ".repeat(usize::from(HORIZONTAL_PADDING))));
    spans.push(Span::styled("│", border_style));
    Line::from(spans)
}

fn startup_banner_border_line(
    content_width: usize,
    left: char,
    right: char,
    palette: TerminalPalette,
) -> Line<'static> {
    let horizontal_width = content_width + usize::from(HORIZONTAL_PADDING * 2);
    Line::styled(
        format!("{left}{}{right}", "─".repeat(horizontal_width)),
        style_for_color(palette.secondary),
    )
}

fn append_text_as_styled_spans(
    spans: &mut Vec<Span<'static>>,
    current_width: &mut usize,
    text: &str,
    style: Style,
) {
    for cluster in UnicodeSegmentation::graphemes(text, true) {
        if cluster == "\t" {
            let tab_width = display_tab_width(*current_width);
            push_styled_span(spans, " ".repeat(tab_width), style);
            *current_width += tab_width;
        } else {
            push_styled_span(spans, cluster.to_string(), style);
            *current_width += display_width(cluster);
        }
    }
}

fn append_text_for_plain_row(plain_text: &mut String, current_width: &mut usize, text: &str) {
    for cluster in UnicodeSegmentation::graphemes(text, true) {
        if cluster == "\t" {
            let tab_width = display_tab_width(*current_width);
            plain_text.push_str(&" ".repeat(tab_width));
            *current_width += tab_width;
        } else {
            plain_text.push_str(cluster);
            *current_width += display_width(cluster);
        }
    }
}

fn push_styled_span(spans: &mut Vec<Span<'static>>, text: String, style: Style) {
    if text.is_empty() {
        return;
    }

    if let Some(last_span) = spans.last_mut()
        && last_span.style == style
    {
        last_span.content.to_mut().push_str(&text);
        return;
    }

    spans.push(Span::styled(text, style));
}

fn startup_banner_style_for_role(role: StartupBannerStyleRole, palette: TerminalPalette) -> Style {
    match role {
        StartupBannerStyleRole::Main => style_for_color(palette.main),
        StartupBannerStyleRole::Secondary => style_for_color(palette.secondary),
        StartupBannerStyleRole::Tertiary => style_for_color(palette.tertiary),
        StartupBannerStyleRole::CommandAccent => style_for_color(palette.command_accent),
        StartupBannerStyleRole::Reset => Style::new(),
    }
}

fn style_for_color(color: Color) -> Style {
    if color == Color::Reset {
        Style::new()
    } else {
        Style::new().fg(color)
    }
}

fn to_u16_width(width: usize) -> u16 {
    u16::try_from(width).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_APP_NAME, DEFAULT_VERSION, StartupBannerOptions, render_startup_banner_buffer,
        startup_banner_title_plain_text,
    };
    use crate::display_width::display_width;
    use crate::theme::palette_from_background;

    #[test]
    fn render_keeps_title_width_when_work_dir_is_absent() {
        let buffer =
            render_startup_banner_buffer(&StartupBannerOptions::default(), sample_palette(), "");

        let title = startup_banner_title_plain_text(DEFAULT_APP_NAME, DEFAULT_VERSION);
        assert_eq!(buffer.area.width, display_width(&title) as u16);
        assert_eq!(buffer.area.height, 1);
    }

    fn sample_palette() -> crate::theme::TerminalPalette {
        palette_from_background(true, None)
    }
}
