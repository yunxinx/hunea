use ratatui::text::Line;
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
    let title = default_title();
    let directory_line = format!("directory: {work_dir}");
    let content_width = title.chars().count().max(directory_line.chars().count()) as u16;

    assert_eq!(buffer.area.width, content_width + 4);
    assert_eq!(buffer.area.height, 5);

    assert_eq!(
        buffer_line(&buffer, 0),
        horizontal_border(content_width, '╭', '╮')
    );
    assert_eq!(
        buffer_line(&buffer, 1),
        framed_content_line(&title, content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 2),
        framed_content_line("", content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 3),
        framed_content_line(&directory_line, content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 4),
        horizontal_border(content_width, '╰', '╯')
    );
}

#[test]
fn render_includes_selected_model_and_models_command_hint() {
    let work_dir = "~/GoCodes/lumos_rust";
    let model_line = "model:     gpt-5.5   /models to change";
    let directory_line = format!("directory: {work_dir}");
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            model_name: Some("gpt-5.5".to_string()),
            work_dir: Some(work_dir.to_string()),
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );
    let content_width = default_title()
        .chars()
        .count()
        .max(model_line.chars().count())
        .max(directory_line.chars().count()) as u16;

    assert_eq!(buffer.area.width, content_width + 4);
    assert_eq!(buffer.area.height, 6);
    assert_eq!(
        buffer_line(&buffer, 0),
        horizontal_border(content_width, '╭', '╮')
    );
    assert_eq!(
        buffer_line(&buffer, 1),
        framed_content_line(&default_title(), content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 2),
        framed_content_line("", content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 3),
        framed_content_line(model_line, content_width)
    );
    assert_eq!(
        buffer_line(&buffer, 4),
        framed_content_line(&directory_line, content_width)
    );
}

#[test]
fn render_without_model_or_directory_uses_title_only_without_frame_or_background() {
    let palette = sample_palette();
    let title = default_title();
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            work_dir: Some(String::new()),
            ..StartupBannerOptions::default()
        },
        palette,
    );

    assert_eq!(buffer.area.width, title.chars().count() as u16);
    assert_eq!(buffer.area.height, 1);
    assert_eq!(buffer_line(&buffer, 0), title);
    assert!(
        buffer_line(&buffer, 0).chars().all(|character| {
            !matches!(character, '╭' | '╮' | '╰' | '╯' | '│' | '─')
        })
    );
    for column in 0..buffer.area.width {
        assert_eq!(buffer[(column, 0)].bg, ratatui::style::Color::Reset);
    }
}

#[test]
fn render_title_only_uses_ratatui_display_width_for_wide_glyphs() {
    let app_name = "你好";
    let title = format!("{app_name} (v{})", env!("CARGO_PKG_VERSION"));
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            app_name: Some(app_name.to_string()),
            work_dir: Some(String::new()),
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, display_width(&title));
    assert_eq!(buffer[(0, 0)].symbol(), "你");
    assert_eq!(
        buffer[(1, 0)].symbol(),
        " ",
        "wide glyph continuation cell should be cleared by Ratatui rendering"
    );
    assert_eq!(buffer[(2, 0)].symbol(), "好");
}

#[test]
fn render_framed_banner_uses_display_width_for_unicode_content() {
    let app_name = "你好";
    let model_name = "模型";
    let work_dir = "~/项目";
    let title = format!("{app_name} (v{})", env!("CARGO_PKG_VERSION"));
    let model_line = format!("model:     {model_name}   /models to change");
    let directory_line = format!("directory: {work_dir}");
    let content_width = display_width(&title)
        .max(display_width(&model_line))
        .max(display_width(&directory_line));

    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            app_name: Some(app_name.to_string()),
            model_name: Some(model_name.to_string()),
            work_dir: Some(work_dir.to_string()),
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, content_width + 4);
    assert_eq!(buffer[(0, 1)].symbol(), "│");
    assert_eq!(buffer[(buffer.area.width - 1, 1)].symbol(), "│");
    assert_eq!(buffer[(2, 1)].symbol(), "你");
    assert_eq!(
        buffer[(3, 1)].symbol(),
        " ",
        "framed title should leave Ratatui's wide glyph continuation cell intact"
    );
    assert_eq!(buffer[(4, 1)].symbol(), "好");
}

#[test]
fn render_title_only_expands_tabs_before_measuring_and_writing() {
    let app_name = "Hu\tnea";
    let title = format!("Hu{}nea (v{})", " ".repeat(6), env!("CARGO_PKG_VERSION"));
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            app_name: Some(app_name.to_string()),
            work_dir: Some(String::new()),
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, display_width(&title));
    assert!(!buffer_line(&buffer, 0).contains('\t'));
    assert_eq!(buffer_line(&buffer, 0), title);
}

#[test]
fn render_applies_codex_like_palette_roles_to_startup_banner_fragments() {
    let palette = sample_palette();
    let work_dir = "~/GoCodes/lumos_rust";
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            model_name: Some("gpt-5.5".to_string()),
            work_dir: Some(work_dir.to_string()),
            ..StartupBannerOptions::default()
        },
        palette,
    );

    assert_eq!(buffer[(0, 0)].fg, palette.secondary);
    assert_eq!(buffer[(1, 0)].fg, palette.secondary);
    let title_row = 1;
    let title_start = 2;
    let title_end = title_start + default_title().chars().count() as u16;
    assert_eq!(
        buffer[(title_start - 1, title_row)].bg,
        ratatui::style::Color::Reset
    );
    assert_eq!(
        buffer[(title_start, title_row)].bg,
        ratatui::style::Color::Reset
    );
    assert_eq!(buffer[(title_start, title_row)].fg, palette.main);
    assert_eq!(buffer[(title_start + 4, title_row)].fg, palette.main);
    assert_eq!(buffer[(title_start + 6, title_row)].fg, palette.secondary);
    assert_eq!(
        buffer[(title_end - 1, title_row)].bg,
        ratatui::style::Color::Reset
    );
    assert_eq!(
        buffer[(title_end, title_row)].bg,
        ratatui::style::Color::Reset
    );

    let model_row = 3;
    let content_start = 2;
    let model_value_column = content_start + "model:     ".chars().count() as u16;
    let models_hint_column = content_start + "model:     gpt-5.5   ".chars().count() as u16;
    let hint_suffix_column = models_hint_column + "/models ".chars().count() as u16;

    assert_eq!(buffer[(content_start, model_row)].fg, palette.tertiary);
    assert_eq!(buffer[(model_value_column, model_row)].fg, palette.main);
    assert_eq!(
        buffer[(models_hint_column, model_row)].fg,
        palette.command_accent
    );
    assert_eq!(buffer[(hint_suffix_column, model_row)].fg, palette.tertiary);

    let directory_row = 4;
    let directory_value_column = content_start + "directory: ".chars().count() as u16;
    assert_eq!(buffer[(content_start, directory_row)].fg, palette.tertiary);
    assert_eq!(
        buffer[(directory_value_column, directory_row)].fg,
        palette.main
    );
    assert_eq!(
        buffer[(
            directory_value_column + work_dir.chars().count() as u16 - 1,
            directory_row
        )]
            .fg,
        palette.main
    );
    assert_eq!(
        buffer[(buffer.area.width - 1, directory_row)].fg,
        palette.secondary
    );
}

#[test]
fn render_expands_to_requested_content_width() {
    let work_dir = short_work_dir();
    assert!(
        !work_dir.is_empty(),
        "startup banner test requires a working directory"
    );

    let directory_line = format!("directory: {work_dir}");
    let requested_width = (directory_line.chars().count() as u16).max(24) + 4;
    let buffer = render_startup_banner_buffer_with_palette(
        &StartupBannerOptions {
            width: requested_width,
            ..StartupBannerOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, requested_width + 4);
    assert_eq!(
        buffer_line(&buffer, 3),
        framed_content_line(&directory_line, requested_width)
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

fn default_title() -> String {
    format!("Hunea (v{})", env!("CARGO_PKG_VERSION"))
}

fn horizontal_border(content_width: u16, left: char, right: char) -> String {
    format!("{left}{}{right}", "─".repeat(content_width as usize + 2))
}

fn framed_content_line(content: &str, content_width: u16) -> String {
    format!(
        "│ {content}{padding} │",
        padding = " ".repeat(content_width as usize - content.chars().count())
    )
}

fn buffer_line(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
    (0..buffer.area.width)
        .map(|column| buffer[(column, row)].symbol())
        .collect()
}

fn display_width(text: &str) -> u16 {
    u16::try_from(Line::from(text.to_string()).width()).expect("test width should fit in u16")
}
