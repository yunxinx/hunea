use mo_tui::theme::{
    palette_from_background, system_error_text_style, terminal_default_palette, tertiary_text_style,
};
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

#[test]
fn palette_exposes_a_distinct_tertiary_slot() {
    let palette = palette_from_background(true, Some(Color::Rgb(32, 64, 96)));

    assert_ne!(palette.tertiary, palette.secondary);
}

#[test]
fn tertiary_text_style_uses_the_tertiary_palette_slot() {
    let palette = palette_from_background(true, Some(Color::Rgb(32, 64, 96)));

    assert_eq!(tertiary_text_style(palette).fg, Some(palette.tertiary));
}

#[test]
fn system_error_text_style_uses_the_system_error_palette_slot() {
    let palette = palette_from_background(true, Some(Color::Rgb(32, 64, 96)));

    assert_eq!(
        system_error_text_style(palette).fg,
        Some(palette.system_error)
    );
    assert_ne!(palette.system_error, palette.main);
}
