use lumos::frontend::tui::theme::{palette_from_background, terminal_default_palette};
use ratatui::style::Color;

#[test]
fn palette_keeps_secondary_color_stable_across_dark_backgrounds() {
    let blue_palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));
    let green_palette = palette_from_background(true, Some(Color::Rgb(20, 48, 31)));

    assert_eq!(blue_palette.secondary, green_palette.secondary);
}

#[test]
fn palette_surface_preserves_the_background_hue_direction() {
    let palette = palette_from_background(true, Some(Color::Rgb(32, 64, 96)));

    match palette.surface {
        Some(Color::Rgb(red, green, blue)) => assert!(blue > green && green > red),
        other => panic!("expected rgb surface color, got {other:?}"),
    }
}

#[test]
fn terminal_default_palette_reports_terminal_default_mode() {
    let palette = terminal_default_palette();

    assert!(palette.uses_terminal_default_colors());
}
