use lumos::{
    startup::{HeroOptions, render_hero_buffer_with_palette, write_hero_to},
    theme::TerminalPalette,
};
use ratatui::style::Color;

#[test]
fn render_uses_default_hero_copy_and_rounded_frame() {
    let buffer = render_hero_buffer_with_palette(&HeroOptions::default(), sample_palette());

    assert_eq!(buffer.area.width, 23);
    assert_eq!(buffer.area.height, 3);

    assert_eq!(buffer_line(&buffer, 0), "╭─────────────────────╮");
    assert_eq!(buffer_line(&buffer, 1), "│  >_ Lumos (v0.0.1)  │");
    assert_eq!(buffer_line(&buffer, 2), "╰─────────────────────╯");
}

#[test]
fn render_applies_semantic_colors_to_hero_fragments() {
    let palette = sample_palette();
    let buffer = render_hero_buffer_with_palette(&HeroOptions::default(), palette);

    assert_eq!(buffer[(0, 1)].fg, palette.secondary);
    assert_eq!(buffer[(3, 1)].fg, palette.secondary);
    assert_eq!(buffer[(4, 1)].fg, palette.secondary);
    assert_eq!(buffer[(6, 1)].fg, palette.main);
    assert_eq!(buffer[(10, 1)].fg, palette.main);
    assert_eq!(buffer[(12, 1)].fg, palette.secondary);
    assert_eq!(buffer[(19, 1)].fg, palette.secondary);
    assert_eq!(buffer[(22, 1)].fg, palette.secondary);
}

#[test]
fn render_expands_to_requested_content_width() {
    let buffer = render_hero_buffer_with_palette(
        &HeroOptions {
            width: 24,
            ..HeroOptions::default()
        },
        sample_palette(),
    );

    assert_eq!(buffer.area.width, 30);
    assert_eq!(buffer_line(&buffer, 1), "│  >_ Lumos (v0.0.1)         │");
}

#[test]
fn write_hero_to_appends_a_trailing_newline() {
    let mut output = Vec::new();

    write_hero_to(&mut output, &HeroOptions::default()).expect("hero should render");

    let rendered = String::from_utf8(output).expect("hero output should be utf-8");
    assert!(rendered.ends_with('\n'));
}

fn sample_palette() -> TerminalPalette {
    TerminalPalette {
        main: Color::Rgb(245, 245, 245),
        secondary: Color::Rgb(168, 168, 168),
    }
}

fn buffer_line(buffer: &ratatui::buffer::Buffer, row: u16) -> String {
    (0..buffer.area.width)
        .map(|column| buffer[(column, row)].symbol())
        .collect()
}
