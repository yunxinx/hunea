use runtime_domain::envinfo::short_work_dir;
use terminal_ui::{
    StartupBannerOptions, render_startup_banner_buffer_with_palette,
    theme::{TerminalPalette, palette_from_background},
    write_startup_banner_to,
};

#[test]
fn render_uses_default_startup_banner_copy_and_includes_the_current_working_directory() {
    let work_dir = short_work_dir();
    assert!(
        !work_dir.is_empty(),
        "startup banner test requires a working directory"
    );

    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions::default(),
        sample_palette(),
    );
    let content_width = default_title()
        .chars()
        .count()
        .max(work_dir.chars().count()) as u16;

    assert_eq!(buffer.area.width, content_width + 6);
    assert_eq!(buffer.area.height, 5);

    assert_eq!(
        buffer_line(&buffer, 0),
        horizontal_border(content_width, '╭', '╮')
    );
    assert_eq!(
        buffer_line(&buffer, 1),
        framed_content_line(default_title(), content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 2),
        framed_content_line("", content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 3),
        framed_content_line(&work_dir, content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 4),
        horizontal_border(content_width, '╰', '╯')
    );
}

#[test]
fn render_applies_semantic_colors_to_startup_banner_fragments() {
    let palette = sample_palette();
    let buffer =
        render_startup_banner_buffer_with_palette(&StartupBannerOptions::default(), palette);
    let work_dir = short_work_dir();
    assert!(
        !work_dir.is_empty(),
        "startup banner test requires a working directory"
    );

    assert_eq!(buffer[(0, 1)].fg, palette.secondary);
    assert_eq!(buffer[(3, 1)].fg, palette.secondary);
    assert_eq!(buffer[(4, 1)].fg, palette.secondary);
    assert_eq!(buffer[(6, 1)].fg, palette.main);
    assert_eq!(buffer[(10, 1)].fg, palette.main);
    assert_eq!(buffer[(12, 1)].fg, palette.secondary);
    assert_eq!(buffer[(19, 1)].fg, palette.secondary);
    assert_eq!(buffer[(3, 3)].fg, palette.secondary);
    assert_eq!(
        buffer[(3 + work_dir.chars().count() as u16 - 1, 3)].fg,
        palette.secondary
    );
    assert_eq!(buffer[(buffer.area.width - 1, 3)].fg, palette.secondary);
}

#[test]
fn render_expands_to_requested_content_width() {
    let work_dir = short_work_dir();
    assert!(
        !work_dir.is_empty(),
        "startup banner test requires a working directory"
    );

    let requested_width = (work_dir.chars().count() as u16).max(24) + 4;
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            width: requested_width,
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, requested_width + 6);
    assert_eq!(
        buffer_line(&buffer, 1),
        framed_content_line(default_title(), requested_width)
    );
    assert_eq!(
        buffer_line(&buffer, 3),
        framed_content_line(&work_dir, requested_width)
    );
}

#[test]
fn write_startup_banner_to_appends_a_trailing_newline() {
    let mut output = Vec::new();

    write_startup_banner_to(&mut output, &StartupBannerOptions::default())
        .expect("startup banner should render");

    let rendered = String::from_utf8(output).expect("startup banner output should be utf-8");
    assert!(rendered.ends_with('\n'));
}

fn sample_palette() -> TerminalPalette {
    palette_from_background(true, None)
}

fn default_title() -> &'static str {
    ">_ Lumos (v0.1.0)"
}

fn horizontal_border(content_width: u16, left: char, right: char) -> String {
    format!("{left}{}{right}", "─".repeat(content_width as usize + 4))
}

fn framed_content_line(content: &str, content_width: u16) -> String {
    format!(
        "│  {content}{padding}  │",
        padding = " ".repeat(content_width as usize - content.chars().count())
    )
}

fn buffer_line(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
    (0..buffer.area.width)
        .map(|column| buffer[(column, row)].symbol())
        .collect()
}
