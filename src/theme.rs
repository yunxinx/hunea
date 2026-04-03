use std::{env, io::IsTerminal};

use ratatui::style::Color;

const DEFAULT_MAIN_TRUECOLOR: RgbColor = RgbColor {
    red: 245,
    green: 245,
    blue: 245,
};
const DARK_BACKGROUND_SECONDARY: RgbColor = RgbColor {
    red: 168,
    green: 168,
    blue: 168,
};
const LIGHT_BACKGROUND_SECONDARY: RgbColor = RgbColor {
    red: 47,
    green: 47,
    blue: 47,
};

/// TerminalPalette 定义当前终端里最基础的两个颜色语义。
/// `main` 用于应用名等主体信息，`secondary` 用于边框、前缀和版本号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub main: Color,
    pub secondary: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalColorContext {
    profile: TerminalColorProfile,
    background: Option<RgbColor>,
    has_dark_background: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalColorProfile {
    NoColor,
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
}

/// DetectPalette 根据终端颜色能力和背景明暗生成一组可复用的基础配色。
pub fn detect_palette() -> TerminalPalette {
    palette_from_context(detect_terminal_color_context())
}

fn palette_from_context(context: TerminalColorContext) -> TerminalPalette {
    TerminalPalette {
        main: main_color(context.profile),
        secondary: secondary_color(context),
    }
}

fn detect_terminal_color_context() -> TerminalColorContext {
    let background = detect_background();

    TerminalColorContext {
        profile: detect_color_profile(),
        has_dark_background: background
            .map(is_dark_background)
            .unwrap_or_else(detect_dark_background_from_env),
        background,
    }
}

fn detect_color_profile() -> TerminalColorProfile {
    if !std::io::stdout().is_terminal() || env::var_os("NO_COLOR").is_some() {
        return TerminalColorProfile::NoColor;
    }

    if let Some(force_color) = env::var_os("FORCE_COLOR") {
        return match force_color.to_string_lossy().trim() {
            "0" => TerminalColorProfile::NoColor,
            "3" => TerminalColorProfile::TrueColor,
            "2" => TerminalColorProfile::Ansi256,
            _ => TerminalColorProfile::Ansi16,
        };
    }

    let color_term = env::var("COLORTERM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(color_term.as_str(), "truecolor" | "24bit") {
        return TerminalColorProfile::TrueColor;
    }

    let term = env::var("TERM").unwrap_or_default().to_ascii_lowercase();
    if term.contains("256color") {
        TerminalColorProfile::Ansi256
    } else if term.is_empty() || term == "dumb" {
        TerminalColorProfile::NoColor
    } else {
        TerminalColorProfile::Ansi16
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

fn main_color(profile: TerminalColorProfile) -> Color {
    match profile {
        TerminalColorProfile::NoColor => Color::Reset,
        TerminalColorProfile::Ansi16 => Color::Gray,
        TerminalColorProfile::Ansi256 => Color::Indexed(15),
        TerminalColorProfile::TrueColor => rgb_to_color(DEFAULT_MAIN_TRUECOLOR),
    }
}

fn secondary_color(context: TerminalColorContext) -> Color {
    let truecolor = if let Some(background) = context.background {
        let complementary = complementary(background);
        if context.has_dark_background {
            lighten(complementary, 0.10)
        } else {
            darken(complementary, 0.10)
        }
    } else if context.has_dark_background {
        DARK_BACKGROUND_SECONDARY
    } else {
        LIGHT_BACKGROUND_SECONDARY
    };

    match context.profile {
        TerminalColorProfile::NoColor => Color::Reset,
        TerminalColorProfile::Ansi16 => Color::Black,
        TerminalColorProfile::Ansi256 => Color::Indexed(238),
        TerminalColorProfile::TrueColor => rgb_to_color(truecolor),
    }
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
        RgbColor, TerminalColorContext, TerminalColorProfile, complementary,
        detect_dark_background_from_env, palette_from_context,
    };
    use ratatui::style::Color;

    #[test]
    fn palette_uses_complementary_secondary_when_background_is_available() {
        let palette = palette_from_context(TerminalColorContext {
            profile: TerminalColorProfile::TrueColor,
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
        let dark_palette = palette_from_context(TerminalColorContext {
            profile: TerminalColorProfile::TrueColor,
            background: None,
            has_dark_background: true,
        });
        let light_palette = palette_from_context(TerminalColorContext {
            profile: TerminalColorProfile::TrueColor,
            background: None,
            has_dark_background: false,
        });

        assert_eq!(dark_palette.secondary, Color::Rgb(168, 168, 168));
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
        (f32::from(channel) + ((255.0 - f32::from(channel)) * 0.10)).round() as u8
    }
}
