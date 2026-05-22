use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use super::theme::TerminalPalette;

const SHIMMER_PADDING: usize = 10;
const SHIMMER_SWEEP_SECONDS: f32 = 2.0;
const SHIMMER_BAND_HALF_WIDTH: f32 = 5.0;

type Rgb = (u8, u8, u8);

/// `shimmer_spans_at` 返回类似 codex-rs 的逐字符扫光文字。
pub(crate) fn shimmer_spans_at(
    text: &str,
    palette: TerminalPalette,
    started_at: Instant,
    now: Instant,
) -> Vec<Span<'static>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }

    let period = chars.len() + SHIMMER_PADDING * 2;
    let elapsed = now.saturating_duration_since(started_at);
    let pos = sweep_position(elapsed, period);
    let rgb_pair = shimmer_rgb_pair(palette);

    chars
        .into_iter()
        .enumerate()
        .map(|(index, ch)| {
            let intensity = shimmer_intensity(index, pos);
            let style = match rgb_pair {
                Some((base_color, highlight_color)) => {
                    let (red, green, blue) =
                        blend(highlight_color, base_color, intensity.clamp(0.0, 1.0) * 0.9);
                    Style::new()
                        .fg(Color::Rgb(red, green, blue))
                        .add_modifier(Modifier::BOLD)
                }
                None => fallback_style_for_intensity(intensity),
            };
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

fn sweep_position(elapsed: Duration, period: usize) -> usize {
    let period = period.max(1);
    let elapsed = elapsed.as_secs_f32() % SHIMMER_SWEEP_SECONDS;
    ((elapsed / SHIMMER_SWEEP_SECONDS) * period as f32) as usize
}

fn shimmer_intensity(index: usize, position: usize) -> f32 {
    let index_position = index as isize + SHIMMER_PADDING as isize;
    let distance = (index_position - position as isize).abs() as f32;
    if distance > SHIMMER_BAND_HALF_WIDTH {
        return 0.0;
    }

    let phase = std::f32::consts::PI * (distance / SHIMMER_BAND_HALF_WIDTH);
    0.5 * (1.0 + phase.cos())
}

fn shimmer_rgb_pair(palette: TerminalPalette) -> Option<(Rgb, Rgb)> {
    let base_color = rgb_from_color(palette.main)?;
    let highlight_color = palette.surface.and_then(rgb_from_color)?;
    Some((base_color, highlight_color))
}

fn rgb_from_color(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        _ => None,
    }
}

fn blend(foreground: Rgb, background: Rgb, alpha: f32) -> Rgb {
    let alpha = alpha.clamp(0.0, 1.0);
    let blend_channel = |foreground: u8, background: u8| {
        (foreground as f32 * alpha + background as f32 * (1.0 - alpha)) as u8
    };

    (
        blend_channel(foreground.0, background.0),
        blend_channel(foreground.1, background.1),
        blend_channel(foreground.2, background.2),
    )
}

fn fallback_style_for_intensity(intensity: f32) -> Style {
    if intensity < 0.2 {
        Style::new().add_modifier(Modifier::DIM)
    } else if intensity < 0.6 {
        Style::new()
    } else {
        Style::new().add_modifier(Modifier::BOLD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{default_palette, terminal_default_palette};

    #[test]
    fn shimmer_spans_preserve_text_while_styles_advance() {
        let started_at = Instant::now();
        let first = shimmer_spans_at("Working", default_palette(), started_at, started_at);
        let second = shimmer_spans_at(
            "Working",
            default_palette(),
            started_at,
            started_at + Duration::from_millis(900),
        );

        assert_eq!(
            first
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "Working"
        );
        assert_eq!(
            second
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "Working"
        );
        assert_ne!(
            first.iter().map(|span| span.style).collect::<Vec<_>>(),
            second.iter().map(|span| span.style).collect::<Vec<_>>()
        );
    }

    #[test]
    fn shimmer_spans_degrade_to_modifier_styles_for_terminal_default_palette() {
        let started_at = Instant::now();
        let spans = shimmer_spans_at(
            "Working",
            terminal_default_palette(),
            started_at,
            started_at,
        );

        assert_eq!(spans.len(), "Working".chars().count());
        assert!(spans.iter().all(|span| span.style.fg.is_none()));
    }
}
