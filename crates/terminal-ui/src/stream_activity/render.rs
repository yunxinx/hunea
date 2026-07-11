use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Modifier, Style},
    text::Span,
};

use super::{STREAM_ACTIVITY_GLYPH, state::StreamActivityState};
use crate::{
    display_width::char_display_width,
    shimmer::shimmer_spans_at,
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, secondary_text_style},
};

const STREAM_ACTIVITY_GLYPH_BREATH_PERIOD_SECS: f32 = 1.6;

type Rgb = (u8, u8, u8);

pub(super) fn render_activity_content(
    activity: &StreamActivityState,
    palette: TerminalPalette,
    now: Instant,
    content_width: usize,
    motion_mode: crate::MotionMode,
) -> (String, Vec<Span<'static>>) {
    let elapsed_text = if motion_mode.allows_animation() {
        activity.elapsed_segment_at(now)
    } else {
        activity.reduced_segment_at(now)
    };
    let text = format!(
        "{STREAM_ACTIVITY_GLYPH} {} {elapsed_text}",
        activity.display_header()
    );
    let truncated_text = truncate_display_width_with_ellipsis(&text, content_width);
    if truncated_text.is_empty() {
        return (String::new(), Vec::new());
    }

    if truncated_text == text {
        return (
            text,
            activity_content_spans(activity, palette, now, elapsed_text, motion_mode),
        );
    }

    (
        truncated_text.clone(),
        truncate_activity_spans(
            activity_content_spans(activity, palette, now, elapsed_text, motion_mode),
            content_width,
        ),
    )
}

fn activity_content_spans(
    activity: &StreamActivityState,
    palette: TerminalPalette,
    now: Instant,
    elapsed_text: String,
    motion_mode: crate::MotionMode,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push(if motion_mode.allows_animation() {
        activity_glyph_span_at(palette, activity.started_at, now)
    } else {
        Span::styled(
            STREAM_ACTIVITY_GLYPH,
            activity_glyph_style_for_intensity(palette, 0.5),
        )
    });
    spans.push(Span::raw(" "));
    if motion_mode.allows_animation() {
        spans.extend(shimmer_spans_at(
            activity.display_header(),
            palette,
            activity.started_at,
            now,
        ));
    } else {
        spans.push(Span::styled(
            activity.display_header().to_string(),
            secondary_text_style(palette),
        ));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        elapsed_text,
        secondary_text_style(palette).dim(),
    ));
    spans
}

fn truncate_activity_spans(spans: Vec<Span<'static>>, content_width: usize) -> Vec<Span<'static>> {
    if content_width == 0 {
        return Vec::new();
    }
    if content_width == 1 {
        return vec![Span::styled("…", secondary_ellipsis_style(&spans))];
    }

    let mut truncated = Vec::new();
    let mut used_width = 0usize;
    let target_width = content_width.saturating_sub(1);
    let mut ellipsis_style = Style::new();

    'outer: for span in spans {
        ellipsis_style = span.style;
        for ch in span.content.chars() {
            let width = char_display_width(ch);
            if used_width.saturating_add(width) > target_width {
                break 'outer;
            }
            used_width += width;
            truncated.push(Span::styled(ch.to_string(), span.style));
        }
    }

    truncated.push(Span::styled("…", ellipsis_style));
    truncated
}

pub(super) fn activity_glyph_span_at(
    palette: TerminalPalette,
    started_at: Instant,
    now: Instant,
) -> Span<'static> {
    let intensity = activity_glyph_intensity(now.saturating_duration_since(started_at));
    Span::styled(
        STREAM_ACTIVITY_GLYPH,
        activity_glyph_style_for_intensity(palette, intensity),
    )
}

fn activity_glyph_intensity(elapsed: Duration) -> f32 {
    let phase = (elapsed.as_secs_f32() % STREAM_ACTIVITY_GLYPH_BREATH_PERIOD_SECS)
        / STREAM_ACTIVITY_GLYPH_BREATH_PERIOD_SECS;
    0.5 * (1.0 - (phase * std::f32::consts::TAU).cos())
}

fn activity_glyph_style_for_intensity(palette: TerminalPalette, intensity: f32) -> Style {
    let intensity = intensity.clamp(0.0, 1.0);
    match activity_glyph_rgb_pair(palette) {
        Some((base_color, highlight_color)) => {
            let alpha = 0.2 + intensity * 0.8;
            let (red, green, blue) = blend_rgb(highlight_color, base_color, alpha);
            let style = Style::new().fg(Color::Rgb(red, green, blue));
            if intensity >= 0.55 {
                style.add_modifier(Modifier::BOLD)
            } else if intensity <= 0.2 {
                style.add_modifier(Modifier::DIM)
            } else {
                style
            }
        }
        None => fallback_activity_glyph_style(intensity),
    }
}

fn activity_glyph_rgb_pair(palette: TerminalPalette) -> Option<(Rgb, Rgb)> {
    Some((
        rgb_from_color(palette.tertiary)?,
        rgb_from_color(palette.main)?,
    ))
}

fn rgb_from_color(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(red, green, blue) => Some((red, green, blue)),
        _ => None,
    }
}

fn blend_rgb(foreground: Rgb, background: Rgb, alpha: f32) -> Rgb {
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

fn fallback_activity_glyph_style(intensity: f32) -> Style {
    if intensity <= 0.2 {
        Style::new().add_modifier(Modifier::DIM)
    } else if intensity >= 0.55 {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    }
}

fn secondary_ellipsis_style(spans: &[Span<'static>]) -> Style {
    spans.last().map(|span| span.style).unwrap_or_default()
}
