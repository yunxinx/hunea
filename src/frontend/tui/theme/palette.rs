use std::env;

use ratatui::style::Color;

const DARK_BACKGROUND_MAIN: RgbColor = RgbColor {
    red: 245,
    green: 245,
    blue: 245,
};
const LIGHT_BACKGROUND_MAIN: RgbColor = RgbColor {
    red: 47,
    green: 47,
    blue: 47,
};
const DARK_BACKGROUND_SECONDARY: RgbColor = RgbColor {
    red: 191,
    green: 191,
    blue: 191,
};
const LIGHT_BACKGROUND_SECONDARY: RgbColor = RgbColor {
    red: 47,
    green: 47,
    blue: 47,
};

/// `TerminalPalette` 定义当前终端里最基础的两个颜色语义。
/// `main` 用于应用名等主体信息，`secondary` 用于边框、前缀和版本号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub main: Color,
    pub secondary: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PaletteContext {
    background: Option<RgbColor>,
    has_dark_background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
}

/// `detect_palette` 根据终端背景明暗生成一组可复用的基础配色。
/// 颜色统一保留为 RGB 语义，不在这里维护终端颜色档位降级分支。
pub fn detect_palette() -> TerminalPalette {
    palette_from_context(detect_palette_context())
}

fn palette_from_context(context: PaletteContext) -> TerminalPalette {
    TerminalPalette {
        main: main_color(context.has_dark_background),
        secondary: secondary_color(context),
    }
}

fn detect_palette_context() -> PaletteContext {
    let background = detect_background();

    PaletteContext {
        has_dark_background: background
            .map(is_dark_background)
            .unwrap_or_else(detect_dark_background_from_env),
        background,
    }
}

fn detect_background() -> Option<RgbColor> {
    let color = terminal_light::background_color().ok()?;

    match color {
        terminal_light::Color::Rgb(rgb) => Some(RgbColor {
            red: rgb.r,
            green: rgb.g,
            blue: rgb.b,
        }),
        _ => None,
    }
}

fn detect_dark_background_from_env() -> bool {
    let Some(value) = env::var_os("COLORFGBG") else {
        return true;
    };

    let background_code = value
        .to_string_lossy()
        .split(';')
        .next_back()
        .and_then(|segment| segment.parse::<u8>().ok());

    match background_code {
        Some(code) if code <= 6 => true,
        Some(code) if code >= 8 => false,
        Some(_) => false,
        None => true,
    }
}

fn main_color(has_dark_background: bool) -> Color {
    if has_dark_background {
        rgb_to_color(DARK_BACKGROUND_MAIN)
    } else {
        rgb_to_color(LIGHT_BACKGROUND_MAIN)
    }
}

fn secondary_color(context: PaletteContext) -> Color {
    let secondary = if let Some(background) = context.background {
        let complementary = complementary(background);
        if context.has_dark_background {
            lighten(complementary, 0.20)
        } else {
            darken(complementary, 0.10)
        }
    } else if context.has_dark_background {
        DARK_BACKGROUND_SECONDARY
    } else {
        LIGHT_BACKGROUND_SECONDARY
    };

    rgb_to_color(secondary)
}

fn complementary(color: RgbColor) -> RgbColor {
    RgbColor {
        red: 255 - color.red,
        green: 255 - color.green,
        blue: 255 - color.blue,
    }
}

fn lighten(color: RgbColor, amount: f32) -> RgbColor {
    blend_toward(
        color,
        RgbColor {
            red: 255,
            green: 255,
            blue: 255,
        },
        amount,
    )
}

fn darken(color: RgbColor, amount: f32) -> RgbColor {
    blend_toward(
        color,
        RgbColor {
            red: 0,
            green: 0,
            blue: 0,
        },
        amount,
    )
}

fn blend_toward(from: RgbColor, to: RgbColor, amount: f32) -> RgbColor {
    fn blend_channel(from: u8, to: u8, amount: f32) -> u8 {
        let from = from as f32;
        let to = to as f32;
        (from + ((to - from) * amount)).round() as u8
    }

    RgbColor {
        red: blend_channel(from.red, to.red, amount),
        green: blend_channel(from.green, to.green, amount),
        blue: blend_channel(from.blue, to.blue, amount),
    }
}

fn is_dark_background(color: RgbColor) -> bool {
    let luma = (0.2126 * f32::from(color.red))
        + (0.7152 * f32::from(color.green))
        + (0.0722 * f32::from(color.blue));

    luma < 140.0
}

fn rgb_to_color(color: RgbColor) -> Color {
    Color::Rgb(color.red, color.green, color.blue)
}

#[cfg(test)]
mod tests {
    use super::{
        PaletteContext, RgbColor, complementary, detect_dark_background_from_env,
        palette_from_context,
    };
    use ratatui::style::Color;

    #[test]
    fn palette_uses_complementary_secondary_when_background_is_available() {
        let palette = palette_from_context(PaletteContext {
            background: Some(RgbColor {
                red: 16,
                green: 32,
                blue: 48,
            }),
            has_dark_background: true,
        });

        let complementary = complementary(RgbColor {
            red: 16,
            green: 32,
            blue: 48,
        });

        assert_eq!(palette.main, Color::Rgb(245, 245, 245));
        assert_eq!(
            palette.secondary,
            Color::Rgb(
                lightened_channel(complementary.red),
                lightened_channel(complementary.green),
                lightened_channel(complementary.blue),
            )
        );
    }

    #[test]
    fn palette_uses_background_sensitive_fallback_without_background_color() {
        let dark_palette = palette_from_context(PaletteContext {
            background: None,
            has_dark_background: true,
        });
        let light_palette = palette_from_context(PaletteContext {
            background: None,
            has_dark_background: false,
        });

        assert_eq!(dark_palette.main, Color::Rgb(245, 245, 245));
        assert_eq!(dark_palette.secondary, Color::Rgb(191, 191, 191));
        assert_eq!(light_palette.main, Color::Rgb(47, 47, 47));
        assert_eq!(light_palette.secondary, Color::Rgb(47, 47, 47));
    }

    #[test]
    fn colorfgbg_defaults_to_dark_when_the_value_is_missing() {
        unsafe {
            std::env::remove_var("COLORFGBG");
        }

        assert!(detect_dark_background_from_env());
    }

    fn lightened_channel(channel: u8) -> u8 {
        (f32::from(channel) + ((255.0 - f32::from(channel)) * 0.20)).round() as u8
    }
}
