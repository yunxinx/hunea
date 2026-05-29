use std::env;

use ratatui::style::Color;

const DARK_BACKGROUND_MAIN: Color = Color::Rgb(245, 245, 245);
const LIGHT_BACKGROUND_MAIN: Color = Color::Rgb(47, 47, 47);
const DARK_BACKGROUND_MUTED: Color = Color::Rgb(202, 202, 202);
const LIGHT_BACKGROUND_MUTED: Color = Color::Rgb(95, 95, 95);
const DARK_BACKGROUND_SECONDARY: Color = Color::Rgb(166, 166, 166);
const LIGHT_BACKGROUND_SECONDARY: Color = Color::Rgb(110, 110, 110);
const DARK_BACKGROUND_TERTIARY: Color = Color::Rgb(154, 154, 154);
const LIGHT_BACKGROUND_TERTIARY: Color = Color::Rgb(122, 122, 122);
const DARK_BACKGROUND_QUOTE: Color = Color::Rgb(166, 220, 176);
const LIGHT_BACKGROUND_QUOTE: Color = Color::Rgb(70, 128, 82);
const DARK_BACKGROUND_TABLE_HEADER: Color = Color::Rgb(137, 180, 250);
const LIGHT_BACKGROUND_TABLE_HEADER: Color = Color::Rgb(37, 99, 160);
const DARK_BACKGROUND_APPROVAL_REJECTED: Color = Color::Rgb(245, 213, 130);
const LIGHT_BACKGROUND_APPROVAL_REJECTED: Color = Color::Rgb(150, 115, 42);
const DARK_BACKGROUND_SURFACE: Color = Color::Rgb(46, 46, 46);
const LIGHT_BACKGROUND_SURFACE: Color = Color::Rgb(236, 236, 236);
const DARK_BACKGROUND_SYSTEM_ERROR: Color = Color::Rgb(255, 153, 153);
const LIGHT_BACKGROUND_SYSTEM_ERROR: Color = Color::Rgb(188, 74, 74);
const PANEL_ACCENT: Color = Color::Blue;
const COMMAND_ACCENT: Color = Color::Cyan;

/// `PaletteDetection` 描述一次可确认的终端背景探测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteDetection {
    pub palette: TerminalPalette,
    pub has_dark_background: bool,
}

/// `TerminalPalette` 定义当前终端下的一组基础语义颜色。
/// `main` 用于主体信息，`muted` 用于输入正文，`secondary` 用于辅助信息，
/// `tertiary` 用于更弱的状态信息，`accent` 用于面板强调线，
/// `command_accent` 用于斜杠菜单当前命令，`approval_rejected` 用于人为拒绝审批，
/// `system_error` 用于运行时错误提示，
/// `quote` 用于 Markdown 引用块，`table_header` 用于 Markdown 表头，
/// `surface` 用于弱化背景块。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalPalette {
    pub main: Color,
    pub muted: Color,
    pub secondary: Color,
    pub tertiary: Color,
    pub accent: Color,
    pub command_accent: Color,
    pub approval_rejected: Color,
    pub system_error: Color,
    pub quote: Color,
    pub table_header: Color,
    pub surface: Option<Color>,
    mode: PaletteMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PaletteMode {
    Explicit,
    TerminalDefault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RgbColor {
    red: u8,
    green: u8,
    blue: u8,
}

/// `default_palette` 返回不触发终端探测的稳定默认配色。
pub fn default_palette() -> TerminalPalette {
    palette_from_background(true, None)
}

/// `terminal_default_palette` 返回依赖终端默认颜色的保底配色。
pub fn terminal_default_palette() -> TerminalPalette {
    TerminalPalette {
        main: Color::Reset,
        muted: Color::Reset,
        secondary: Color::Reset,
        tertiary: Color::Reset,
        accent: PANEL_ACCENT,
        command_accent: COMMAND_ACCENT,
        approval_rejected: Color::LightYellow,
        system_error: Color::LightRed,
        quote: Color::LightGreen,
        table_header: Color::Cyan,
        surface: None,
        mode: PaletteMode::TerminalDefault,
    }
}

impl TerminalPalette {
    /// `uses_terminal_default_colors` 表示当前配色是否依赖终端默认前景/背景色。
    pub fn uses_terminal_default_colors(&self) -> bool {
        matches!(self.mode, PaletteMode::TerminalDefault)
    }
}

/// `detect_palette` 用于非交互场景，优先探测真实背景，失败时再回退到环境变量推断。
pub fn detect_palette() -> TerminalPalette {
    detect_palette_from_sources(detect_background, detect_dark_background_from_env)
}

/// `try_detect_palette` 仅在拿到真实终端背景色时返回显式配色。
pub fn try_detect_palette() -> Option<PaletteDetection> {
    let background = detect_background()?;
    let has_dark_background = is_dark_background(background);

    Some(PaletteDetection {
        palette: palette_from_background(has_dark_background, Some(rgb_to_color(background))),
        has_dark_background,
    })
}

fn detect_palette_from_sources(
    background_probe: impl FnOnce() -> Option<RgbColor>,
    dark_background_probe: impl FnOnce() -> bool,
) -> TerminalPalette {
    match background_probe() {
        Some(background) => palette_from_background(
            is_dark_background(background),
            Some(rgb_to_color(background)),
        ),
        None => palette_from_background(dark_background_probe(), None),
    }
}

/// `palette_from_background` 根据终端背景信息生成语义配色。
/// `background` 为 `None` 时会退回到稳定的 fallback 颜色。
pub fn palette_from_background(
    has_dark_background: bool,
    background: Option<Color>,
) -> TerminalPalette {
    TerminalPalette {
        main: if has_dark_background {
            DARK_BACKGROUND_MAIN
        } else {
            LIGHT_BACKGROUND_MAIN
        },
        muted: if has_dark_background {
            DARK_BACKGROUND_MUTED
        } else {
            LIGHT_BACKGROUND_MUTED
        },
        secondary: if has_dark_background {
            DARK_BACKGROUND_SECONDARY
        } else {
            LIGHT_BACKGROUND_SECONDARY
        },
        tertiary: if has_dark_background {
            DARK_BACKGROUND_TERTIARY
        } else {
            LIGHT_BACKGROUND_TERTIARY
        },
        accent: PANEL_ACCENT,
        command_accent: COMMAND_ACCENT,
        approval_rejected: if has_dark_background {
            DARK_BACKGROUND_APPROVAL_REJECTED
        } else {
            LIGHT_BACKGROUND_APPROVAL_REJECTED
        },
        system_error: if has_dark_background {
            DARK_BACKGROUND_SYSTEM_ERROR
        } else {
            LIGHT_BACKGROUND_SYSTEM_ERROR
        },
        quote: if has_dark_background {
            DARK_BACKGROUND_QUOTE
        } else {
            LIGHT_BACKGROUND_QUOTE
        },
        table_header: if has_dark_background {
            DARK_BACKGROUND_TABLE_HEADER
        } else {
            LIGHT_BACKGROUND_TABLE_HEADER
        },
        surface: Some(surface_color(has_dark_background, background)),
        mode: PaletteMode::Explicit,
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

fn surface_color(has_dark_background: bool, background: Option<Color>) -> Color {
    let Some(background) = background.and_then(color_to_rgb) else {
        return if has_dark_background {
            DARK_BACKGROUND_SURFACE
        } else {
            LIGHT_BACKGROUND_SURFACE
        };
    };

    if has_dark_background {
        blend_toward(
            background,
            RgbColor {
                red: 255,
                green: 255,
                blue: 255,
            },
            0.12,
        )
    } else {
        blend_toward(
            background,
            RgbColor {
                red: 0,
                green: 0,
                blue: 0,
            },
            0.04,
        )
    }
}

fn color_to_rgb(color: Color) -> Option<RgbColor> {
    match color {
        Color::Rgb(red, green, blue) => Some(RgbColor { red, green, blue }),
        _ => None,
    }
}

fn blend_toward(from: RgbColor, to: RgbColor, amount: f32) -> Color {
    fn blend_channel(from: u8, to: u8, amount: f32) -> u8 {
        let from = from as f32;
        let to = to as f32;
        (from + ((to - from) * amount)).round() as u8
    }

    Color::Rgb(
        blend_channel(from.red, to.red, amount),
        blend_channel(from.green, to.green, amount),
        blend_channel(from.blue, to.blue, amount),
    )
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
        RgbColor, default_palette, detect_dark_background_from_env, detect_palette_from_sources,
        is_dark_background, palette_from_background, terminal_default_palette,
    };
    use ratatui::style::Color;

    #[test]
    fn palette_uses_stable_secondary_color_without_following_background_hue() {
        let blue_palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));
        let green_palette = palette_from_background(true, Some(Color::Rgb(20, 48, 31)));

        assert_eq!(blue_palette.secondary, green_palette.secondary);
    }

    #[test]
    fn palette_uses_stable_quote_color_without_following_background_hue() {
        let blue_palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));
        let green_palette = palette_from_background(true, Some(Color::Rgb(20, 48, 31)));

        assert_eq!(blue_palette.quote, green_palette.quote);
    }

    #[test]
    fn palette_separates_approval_rejection_from_system_error() {
        let palette = palette_from_background(true, Some(Color::Rgb(16, 36, 63)));

        assert_ne!(palette.approval_rejected, palette.system_error);
    }

    #[test]
    fn palette_table_header_is_a_distinct_readability_accent() {
        let palette = default_palette();

        assert_ne!(palette.table_header, palette.main);
        assert_eq!(terminal_default_palette().table_header, Color::Cyan);
    }

    #[test]
    fn palette_surface_follows_the_background_hue_order() {
        let palette = palette_from_background(true, Some(Color::Rgb(32, 64, 96)));

        assert_eq!(palette.surface, Some(Color::Rgb(59, 87, 115)));
    }

    #[test]
    fn colorfgbg_defaults_to_dark_when_the_value_is_missing() {
        unsafe {
            std::env::remove_var("COLORFGBG");
        }

        assert!(detect_dark_background_from_env());
    }

    #[test]
    fn detect_palette_from_sources_prefers_the_detected_background() {
        let palette = detect_palette_from_sources(
            || {
                Some(RgbColor {
                    red: 32,
                    green: 40,
                    blue: 48,
                })
            },
            || panic!("fallback should not run when background probe succeeds"),
        );

        assert_eq!(
            palette,
            palette_from_background(true, Some(Color::Rgb(32, 40, 48)))
        );
    }

    #[test]
    fn detect_palette_from_sources_falls_back_to_the_dark_probe() {
        let palette = detect_palette_from_sources(|| None, || false);

        assert_eq!(palette, palette_from_background(false, None));
    }

    #[test]
    fn default_palette_uses_the_stable_dark_fallback() {
        assert_eq!(default_palette(), palette_from_background(true, None));
    }

    #[test]
    fn is_dark_background_follows_luma_threshold() {
        assert!(is_dark_background(RgbColor {
            red: 17,
            green: 17,
            blue: 17,
        }));
        assert!(!is_dark_background(RgbColor {
            red: 240,
            green: 240,
            blue: 240,
        }));
    }
}
