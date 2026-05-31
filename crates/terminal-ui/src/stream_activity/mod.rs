//! 运行中 stream activity 的状态与渲染。

use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{
    Model,
    display_width::{char_display_width, display_width},
    selection::SelectableLineRange,
    shimmer::shimmer_spans_at,
    status_line::{StatusLineRenderResult, truncate_display_width_with_ellipsis},
    theme::{TerminalPalette, secondary_text_style},
    tool_result::TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
    transcript::DEFAULT_RENDER_WIDTH,
};

const STREAM_ACTIVITY_FRAME_INTERVAL: Duration = Duration::from_millis(80);
const STREAM_ACTIVITY_TOKEN_TICK_INTERVAL: Duration = Duration::from_millis(33);
const STREAM_ACTIVITY_GLYPH: &str = "•";
const STREAM_ACTIVITY_GLYPH_BREATH_PERIOD_SECS: f32 = 1.6;
const TOKEN_TWEEN_DURATION: Duration = Duration::from_millis(120);
const TOKEN_STALE_THRESHOLD: Duration = Duration::from_millis(360);
const WORK_DURATION_SUMMARY_MIN_ELAPSED_SECS: u64 = 30;

type Rgb = (u8, u8, u8);

/// `StreamActivityState` 保存一次模型 turn 运行中显示在输入框上方的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamActivityState {
    started_at: Instant,
    header: String,
    retry_header: Option<String>,
    interrupt_hint: Option<String>,
    output_tokens: Option<ActivityTokenProgress>,
    is_thinking: bool,
    paused_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivityTokenProgress {
    previous_display: usize,
    target: usize,
    output_total: usize,
    input_total: usize,
    direction: ActivityTokenDirection,
    updated_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActivityTokenDirection {
    Down,
    Up,
}

impl Model {
    pub(crate) fn show_stream_activity(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        let header = self.status_phrase_selector.next_phrase();
        self.show_stream_activity_with_header(header);
    }

    pub(crate) fn show_stream_activity_with_header(&mut self, header: impl Into<String>) {
        let header = header.into().trim().to_string();
        if header.is_empty() {
            return;
        }

        self.stream_activity = Some(StreamActivityState {
            started_at: Instant::now(),
            header,
            retry_header: None,
            interrupt_hint: self.current_stream_activity_interrupt_hint(),
            output_tokens: None,
            is_thinking: false,
            paused_at: None,
        });
        self.reset_chat_interrupt_esc_count();
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn show_stream_activity_retry_header(&mut self, header: impl Into<String>) {
        self.show_stream_activity_retry_header_at(header, Instant::now());
    }

    fn show_stream_activity_retry_header_at(&mut self, header: impl Into<String>, now: Instant) {
        let header = header.into().trim().to_string();
        if header.is_empty() {
            return;
        }

        let Some(activity) = self.stream_activity.as_mut() else {
            self.stream_activity = Some(StreamActivityState {
                started_at: now,
                header: "Working".to_string(),
                retry_header: Some(header),
                interrupt_hint: self.current_stream_activity_interrupt_hint(),
                output_tokens: None,
                is_thinking: false,
                paused_at: Some(now),
            });
            self.reset_chat_interrupt_esc_count();
            self.bump_status_line_revision();
            self.sync_composer_height();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
            }
            return;
        };
        if !activity.enter_retry(header, now) {
            return;
        }

        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn clear_stream_activity_retry_header(&mut self) {
        self.clear_stream_activity_retry_header_at(Instant::now());
    }

    fn clear_stream_activity_retry_header_at(&mut self, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        if !activity.exit_retry(now) {
            return;
        }

        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn current_stream_activity_interrupt_hint(&self) -> Option<String> {
        if !self.show_esc_interrupt_hint {
            return None;
        }

        Some(match self.esc_interrupt_presses {
            1 => "esc to interrupt".to_string(),
            presses => format!("esc {presses}x to interrupt"),
        })
    }

    pub(crate) fn clear_stream_activity(&mut self) {
        if self.stream_activity.is_none() {
            return;
        }

        self.stream_activity = None;
        if self
            .transcript_mut()
            .mark_exploration_tool_activities_complete()
        {
            self.sync_transcript_render();
        }
        self.reset_chat_interrupt_esc_count();
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn finish_stream_activity_with_work_summary(&mut self) {
        self.finish_stream_activity_with_work_summary_at(Instant::now());
    }

    fn finish_stream_activity_with_work_summary_at(&mut self, now: Instant) {
        let duration = self.stream_activity_duration_at(now);
        self.clear_stream_activity();
        if let Some(duration) =
            duration.filter(|duration| should_append_work_duration_summary(*duration))
        {
            self.append_work_duration_from_runtime(duration);
        }
    }

    fn stream_activity_duration_at(&self, now: Instant) -> Option<Duration> {
        let activity = self.stream_activity.as_ref()?;
        Some(activity.elapsed_at(activity.active_now(now)))
    }

    #[cfg(test)]
    pub(crate) fn backdate_stream_activity_started_at_for_test(&mut self, offset: Duration) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.started_at = activity
            .started_at
            .checked_sub(offset)
            .expect("test clock should allow backdating stream activity");
    }

    pub(crate) fn pause_stream_activity(&mut self) {
        self.pause_stream_activity_at(Instant::now());
    }

    fn pause_stream_activity_at(&mut self, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        if activity.paused_at.is_some() {
            return;
        }

        activity.pause_at(now);
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn resume_stream_activity(&mut self) {
        self.resume_stream_activity_at(Instant::now());
    }

    fn resume_stream_activity_at(&mut self, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        if !activity.resume_at(now) {
            return;
        }

        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn set_stream_activity_output_tokens(&mut self, total_tokens: usize) {
        self.set_stream_activity_output_tokens_at(total_tokens, Instant::now());
    }

    pub(crate) fn set_stream_activity_output_tokens_at(
        &mut self,
        total_tokens: usize,
        now: Instant,
    ) {
        self.record_stream_activity_output_tokens_at(total_tokens, now);
    }

    pub(crate) fn set_stream_activity_input_tokens(&mut self, total_tokens: usize) {
        self.set_stream_activity_input_tokens_at(total_tokens, Instant::now());
    }

    pub(crate) fn set_stream_activity_input_tokens_at(
        &mut self,
        total_tokens: usize,
        now: Instant,
    ) {
        self.record_stream_activity_input_tokens_at(total_tokens, now);
    }

    pub(crate) fn set_stream_activity_thinking(&mut self, is_thinking: bool) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        if activity.is_thinking == is_thinking {
            return;
        }
        activity.is_thinking = is_thinking;
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn record_stream_activity_output_tokens_at(&mut self, total_tokens: usize, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.record_output_tokens(total_tokens, now);
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    #[cfg(test)]
    pub(crate) fn add_stream_activity_input_tokens_at(&mut self, token_delta: usize, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.add_input_tokens(token_delta, now);
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn record_stream_activity_input_tokens_at(&mut self, total_tokens: usize, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.record_input_tokens(total_tokens, now);
        self.bump_status_line_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn current_stream_activity_render_result(&self) -> StatusLineRenderResult {
        self.current_stream_activity_render_result_at(Instant::now())
    }

    pub(crate) fn current_stream_activity_render_result_at(
        &self,
        now: Instant,
    ) -> StatusLineRenderResult {
        let Some(activity) = self.stream_activity.as_ref() else {
            return StatusLineRenderResult::default();
        };
        if activity.is_paused() && !activity.has_retry_header() {
            return StatusLineRenderResult::default();
        }

        let width = if self.width == 0 {
            DEFAULT_RENDER_WIDTH
        } else {
            usize::from(self.width)
        };
        let (text, spans) =
            render_activity_content(activity, self.palette, activity.active_now(now), width);
        if text.is_empty() {
            return StatusLineRenderResult::default();
        }

        StatusLineRenderResult {
            line: Some(Line::from(spans)),
            plain_line: text.clone(),
            selectable: SelectableLineRange::new(0, display_width(&text)),
            has_content: true,
            gap_before: 0,
        }
    }

    pub(crate) fn stream_activity_frame_key(&self, now: Instant) -> usize {
        self.stream_activity
            .as_ref()
            .map(|activity| activity.frame_index_at(activity.active_now(now)))
            .unwrap_or(0)
    }

    pub(crate) fn stream_activity_frame_interval_at(&self, now: Instant) -> Option<Duration> {
        self.stream_activity
            .as_ref()
            .filter(|activity| !activity.is_paused())
            .map(|activity| activity.frame_interval_at(now))
    }

    pub(crate) fn tool_activity_frame_key(&self, now: Instant) -> usize {
        self.transcript
            .active_tool_activity_started_at()
            .map(|started_at| {
                let interval_ms = TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL
                    .as_millis()
                    .max(1);
                (now.saturating_duration_since(started_at).as_millis() / interval_ms) as usize
            })
            .unwrap_or(0)
    }

    pub(crate) fn tool_activity_next_frame_deadline_at(&self, now: Instant) -> Option<Instant> {
        let started_at = self.transcript.active_tool_activity_started_at()?;
        let interval = TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL;
        let interval_ms = interval.as_millis().max(1);
        let elapsed_ms = now.saturating_duration_since(started_at).as_millis();
        let next_frame = elapsed_ms / interval_ms + 1;
        let offset_ms = interval_ms.saturating_mul(next_frame);
        let offset = Duration::from_millis(u64::try_from(offset_ms).unwrap_or(u64::MAX));

        started_at
            .checked_add(offset)
            .or_else(|| now.checked_add(interval))
    }
}

impl StreamActivityState {
    fn display_header(&self) -> &str {
        self.retry_header.as_deref().unwrap_or(&self.header)
    }

    fn has_retry_header(&self) -> bool {
        self.retry_header.is_some()
    }

    fn enter_retry(&mut self, header: String, now: Instant) -> bool {
        let mut changed = self.set_retry_header(header);
        changed |= self.clear_attempt_progress();
        if self.paused_at.is_none() {
            self.pause_at(now);
            changed = true;
        }
        changed
    }

    fn exit_retry(&mut self, now: Instant) -> bool {
        if self.retry_header.is_none() {
            return false;
        }

        self.retry_header = None;
        let _ = self.resume_at(now);
        true
    }

    fn set_retry_header(&mut self, header: String) -> bool {
        if self.retry_header.as_deref() == Some(header.as_str()) {
            return false;
        }

        self.retry_header = Some(header);
        true
    }

    fn clear_attempt_progress(&mut self) -> bool {
        let had_progress = self.is_thinking || self.output_tokens.is_some();
        self.is_thinking = false;
        self.output_tokens = None;
        had_progress
    }

    fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    fn active_now(&self, now: Instant) -> Instant {
        self.paused_at.unwrap_or(now)
    }

    fn pause_at(&mut self, now: Instant) {
        self.paused_at = Some(now);
    }

    fn resume_at(&mut self, now: Instant) -> bool {
        let Some(paused_at) = self.paused_at.take() else {
            return false;
        };
        let paused_for = now.saturating_duration_since(paused_at);
        self.shift_activity_clock(paused_for);
        true
    }

    fn shift_activity_clock(&mut self, offset: Duration) {
        if offset.is_zero() {
            return;
        }
        if let Some(started_at) = self.started_at.checked_add(offset) {
            self.started_at = started_at;
        }
        if let Some(progress) = self.output_tokens.as_mut() {
            progress.shift_clock(offset);
        }
    }

    fn elapsed_at(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started_at)
    }

    fn elapsed_text_at(&self, now: Instant) -> String {
        format_elapsed_compact(self.elapsed_at(now).as_secs())
    }

    fn elapsed_segment_at(&self, now: Instant) -> String {
        let elapsed = self.elapsed_text_at(now);
        let token_text = self.token_segment_at(now);
        let mut segments = vec![elapsed];
        if self.is_thinking {
            segments.push("thinking".to_string());
        }
        if let Some(token_text) = token_text {
            segments.push(token_text);
        }
        if let Some(hint) = self.interrupt_hint.as_deref() {
            segments.push(hint.to_string());
        }
        format!("({})", segments.join(" • "))
    }

    fn frame_index_at(&self, now: Instant) -> usize {
        let interval_ms = self.frame_interval_at(now).as_millis().max(1);
        let tick = self.elapsed_at(now).as_millis() / interval_ms;
        let token_display = self.output_tokens_display_at(now);
        (tick as usize)
            .saturating_mul(1_000_003)
            .saturating_add(token_display)
    }

    fn output_tokens_display_at(&self, now: Instant) -> usize {
        self.output_tokens
            .as_ref()
            .map(|progress| progress.display_at(now))
            .unwrap_or(0)
    }

    fn token_segment_at(&self, now: Instant) -> Option<String> {
        let progress = self.output_tokens.as_ref()?;
        let display = progress.display_at(now);
        (display > 0).then(|| {
            format!(
                "{} {} tokens",
                progress.direction.glyph(),
                format_token_count(display)
            )
        })
    }

    fn frame_interval_at(&self, now: Instant) -> Duration {
        if self
            .output_tokens
            .as_ref()
            .is_some_and(|progress| progress.needs_fast_tick_at(now))
        {
            return STREAM_ACTIVITY_TOKEN_TICK_INTERVAL;
        }
        STREAM_ACTIVITY_FRAME_INTERVAL
    }

    fn record_output_tokens(&mut self, total_tokens: usize, now: Instant) {
        let (input_total, target) = self
            .output_tokens
            .as_ref()
            .map(|progress| (progress.input_total, progress.target))
            .unwrap_or((0, 0));
        let output_total = self
            .output_tokens
            .as_ref()
            .map(|progress| progress.output_total.max(total_tokens))
            .unwrap_or(total_tokens);
        let target = target.max(output_total.saturating_add(input_total));
        self.replace_token_progress(
            output_total,
            input_total,
            target,
            ActivityTokenDirection::Down,
            now,
        );
    }

    #[cfg(test)]
    fn add_input_tokens(&mut self, token_delta: usize, now: Instant) {
        if token_delta == 0 {
            return;
        }
        let input_total = self
            .output_tokens
            .as_ref()
            .map(|progress| progress.input_total)
            .unwrap_or(0)
            .saturating_add(token_delta);
        self.record_input_tokens(input_total, now);
    }

    fn record_input_tokens(&mut self, total_tokens: usize, now: Instant) {
        if total_tokens == 0 {
            return;
        }
        let (output_total, input_total, target) = self
            .output_tokens
            .as_ref()
            .map(|progress| (progress.output_total, progress.input_total, progress.target))
            .unwrap_or((0, 0, 0));
        let input_total = input_total.max(total_tokens);
        let target = target.max(output_total.saturating_add(input_total));
        self.replace_token_progress(
            output_total,
            input_total,
            target,
            ActivityTokenDirection::Up,
            now,
        );
    }

    fn replace_token_progress(
        &mut self,
        output_total: usize,
        input_total: usize,
        target: usize,
        direction: ActivityTokenDirection,
        now: Instant,
    ) {
        let current_display = self.output_tokens_display_at(now);
        let target = target.max(current_display);
        self.output_tokens = Some(ActivityTokenProgress {
            previous_display: current_display,
            target,
            output_total,
            input_total,
            direction,
            updated_at: now,
        });
    }
}

impl ActivityTokenProgress {
    fn shift_clock(&mut self, offset: Duration) {
        if let Some(updated_at) = self.updated_at.checked_add(offset) {
            self.updated_at = updated_at;
        }
    }

    fn display_at(&self, now: Instant) -> usize {
        if self.target <= self.previous_display {
            return self.target;
        }

        let elapsed = now.saturating_duration_since(self.updated_at);
        if elapsed >= TOKEN_TWEEN_DURATION {
            return self.target;
        }

        let total_ms = TOKEN_TWEEN_DURATION.as_millis().max(1);
        let elapsed_ms = elapsed.as_millis().max(1);
        let remaining = self.target.saturating_sub(self.previous_display);
        let progressed = (remaining as u128)
            .saturating_mul(elapsed_ms)
            .saturating_add(total_ms - 1)
            / total_ms;
        self.previous_display
            .saturating_add(progressed as usize)
            .min(self.target)
    }

    fn needs_fast_tick_at(&self, now: Instant) -> bool {
        self.display_at(now) < self.target
            && now.saturating_duration_since(self.updated_at) <= TOKEN_STALE_THRESHOLD
    }
}

impl ActivityTokenDirection {
    fn glyph(self) -> &'static str {
        match self {
            Self::Down => "↓",
            Self::Up => "↑",
        }
    }
}

fn render_activity_content(
    activity: &StreamActivityState,
    palette: TerminalPalette,
    now: Instant,
    content_width: usize,
) -> (String, Vec<Span<'static>>) {
    let elapsed_text = activity.elapsed_segment_at(now);
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
            activity_content_spans(activity, palette, now, elapsed_text),
        );
    }

    (
        truncated_text.clone(),
        truncate_activity_spans(
            activity_content_spans(activity, palette, now, elapsed_text),
            content_width,
        ),
    )
}

fn activity_content_spans(
    activity: &StreamActivityState,
    palette: TerminalPalette,
    now: Instant,
    elapsed_text: String,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.push(activity_glyph_span_at(palette, activity.started_at, now));
    spans.push(Span::raw(" "));
    spans.extend(shimmer_spans_at(
        activity.display_header(),
        palette,
        activity.started_at,
        now,
    ));
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

fn activity_glyph_span_at(
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

pub(crate) fn format_elapsed_compact(elapsed_secs: u64) -> String {
    if elapsed_secs < 60 {
        return format!("{elapsed_secs}s");
    }
    if elapsed_secs < 3600 {
        let minutes = elapsed_secs / 60;
        let seconds = elapsed_secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = elapsed_secs / 3600;
    let minutes = (elapsed_secs % 3600) / 60;
    let seconds = elapsed_secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

fn should_append_work_duration_summary(duration: Duration) -> bool {
    duration.as_secs() > WORK_DURATION_SUMMARY_MIN_ELAPSED_SECS
}

fn format_token_count(tokens: usize) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }

    let tenths = (tokens.saturating_mul(10).saturating_add(500)) / 1_000;
    let whole = tenths / 10;
    let fraction = tenths % 10;
    if fraction == 0 {
        format!("{whole}k")
    } else {
        format!("{whole}.{fraction}k")
    }
}

#[cfg(test)]
mod tests;
