use std::time::{Duration, Instant};

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    Model,
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
const TOKEN_TWEEN_DURATION: Duration = Duration::from_millis(120);
const TOKEN_STALE_THRESHOLD: Duration = Duration::from_millis(360);

/// `StreamActivityState` 保存一次模型 turn 运行中显示在输入框上方的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamActivityState {
    started_at: Instant,
    header: String,
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
    // 当前 OpenAI-compatible 流只会上报下行输出；工具结果回传接入后会使用上行方向。
    #[cfg(test)]
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
        activity.record_input_tokens(token_delta, now);
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
        if activity.is_paused() {
            return StatusLineRenderResult::default();
        }

        let width = if self.width == 0 {
            DEFAULT_RENDER_WIDTH
        } else {
            usize::from(self.width)
        };
        let (text, spans) = render_activity_content(activity, self.palette, now, width);
        if text.is_empty() {
            return StatusLineRenderResult::default();
        }

        StatusLineRenderResult {
            line: Some(Line::from(spans)),
            plain_line: text.clone(),
            selectable: SelectableLineRange::new(0, text.width()),
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
    fn record_input_tokens(&mut self, token_delta: usize, now: Instant) {
        if token_delta == 0 {
            return;
        }
        let (output_total, input_total, target) = self
            .output_tokens
            .as_ref()
            .map(|progress| (progress.output_total, progress.input_total, progress.target))
            .unwrap_or((0, 0, 0));
        let input_total = input_total.saturating_add(token_delta);
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
            #[cfg(test)]
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
        activity.header.as_str()
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
    spans.extend(shimmer_spans_at(
        STREAM_ACTIVITY_GLYPH,
        palette,
        activity.started_at,
        now,
    ));
    spans.push(Span::raw(" "));
    spans.extend(shimmer_spans_at(
        activity.header.as_str(),
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
            let width = ch.width().unwrap_or(0);
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

fn secondary_ellipsis_style(spans: &[Span<'static>]) -> Style {
    spans.last().map(|span| span.style).unwrap_or_default()
}

fn format_elapsed_compact(elapsed_secs: u64) -> String {
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
mod tests {
    use ratatui::style::Modifier;

    use super::*;
    use crate::{HeroOptions, theme::default_palette, transcript::TranscriptItem};
    use mo_core::session::{
        RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
    };

    #[test]
    fn stream_activity_tail_cache_key_changes_when_elapsed_text_changes() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        let initial_key = model.stream_activity_frame_key(started_at);
        let later_key =
            model.stream_activity_frame_key(started_at + std::time::Duration::from_millis(1_200));

        assert_ne!(
            initial_key, later_key,
            "activity cache key must change when the visible elapsed timer changes"
        );
    }

    #[test]
    fn stream_activity_line_uses_shimmer_spans_without_changing_plain_text() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        let first = model.current_stream_activity_render_result_at(started_at);
        let second = model.current_stream_activity_render_result_at(
            started_at + std::time::Duration::from_millis(900),
        );
        let first_line = first.line.expect("activity line should render");
        let second_line = second.line.expect("activity line should render");

        assert_eq!(first.plain_line, "• Working (0s • esc 2x to interrupt)");
        assert_eq!(
            first.selectable.content_columns().map(|(start, _)| start),
            Some(0)
        );
        assert_eq!(second.plain_line, first.plain_line);
        assert!(
            first_line.spans.len() > 8,
            "codex-style shimmer should style the running text per character"
        );
        assert!(
            first_line
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            !first_line
                .spans
                .iter()
                .all(|span| span.style.add_modifier.contains(Modifier::ITALIC))
        );
        assert_ne!(
            first_line
                .spans
                .iter()
                .map(|span| span.style)
                .collect::<Vec<_>>(),
            second_line
                .spans
                .iter()
                .map(|span| span.style)
                .collect::<Vec<_>>(),
            "shimmer styles should advance while the visible text stays stable"
        );
    }

    #[test]
    fn clear_stream_activity_completes_open_exploration_marker() {
        let palette = default_palette();
        let mut model = Model::new(HeroOptions::default());
        model.set_palette(palette, true);
        model.show_stream_activity_with_header("Working");
        model.append_runtime_tool_activity_from_runtime(RuntimeToolActivity {
            activity_id: "call-list".to_string(),
            title: "List Directory crates".to_string(),
            kind: RuntimeToolKind::Search,
            status: RuntimeToolActivityStatus::Completed,
            content: vec![RuntimeToolActivityContent::Text("tui/".to_string())],
            locations: Vec::new(),
            raw_input: Some(serde_json::json!({ "path": "crates" }).into()),
            raw_output: Some("tui/".into()),
        });

        assert_eq!(
            first_tool_result_marker_color(&mut model),
            Some(palette.main)
        );

        model.clear_stream_activity();

        assert_eq!(
            first_tool_result_marker_color(&mut model),
            Some(palette.quote)
        );
    }

    fn first_tool_result_marker_color(model: &mut Model) -> Option<ratatui::style::Color> {
        let palette = model.palette;
        let items = model.transcript_mut().items_snapshot();
        let item = items.iter().find_map(|item| match item.as_ref() {
            TranscriptItem::ToolResult(item) => Some(item),
            _ => None,
        })?;
        item.render_lines(80, palette)
            .first()
            .and_then(|line| line.spans.first())
            .and_then(|span| span.style.fg)
    }

    #[test]
    fn stream_activity_pause_hides_and_resume_excludes_paused_duration() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");
        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        let pause_at = started_at + Duration::from_secs(2);
        let resume_at = pause_at + Duration::from_secs(30);

        model.pause_stream_activity_at(pause_at);
        assert!(
            !model
                .current_stream_activity_render_result_at(resume_at)
                .has_content,
            "paused activity should be hidden"
        );
        assert_eq!(model.stream_activity_frame_interval_at(resume_at), None);

        model.resume_stream_activity_at(resume_at);
        let resumed = model
            .current_stream_activity_render_result_at(resume_at + Duration::from_secs(1))
            .plain_line;
        assert!(
            resumed.contains("(3s"),
            "activity should resume from the elapsed time before approval wait: {resumed}"
        );
        assert!(
            !resumed.contains("33s"),
            "approval wait should not be counted into elapsed time: {resumed}"
        );
    }

    #[test]
    fn stream_activity_line_tweens_output_token_estimate_to_target() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(70, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        model.set_stream_activity_output_tokens_at(24, started_at);

        let early = model
            .current_stream_activity_render_result_at(
                started_at + std::time::Duration::from_millis(80),
            )
            .plain_line;
        let settled = model
            .current_stream_activity_render_result_at(
                started_at + std::time::Duration::from_millis(120),
            )
            .plain_line;

        assert!(
            early.contains("tokens"),
            "activity should expose streaming token feedback before settling"
        );
        assert!(
            !early.contains("24 tokens"),
            "token feedback should tween instead of jumping to the target"
        );
        assert!(
            settled.contains("24 tokens"),
            "token feedback should eventually reach the latest target"
        );
    }

    #[test]
    fn stream_activity_token_indicator_uses_single_directional_total() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        model.set_stream_activity_output_tokens_at(200, started_at);
        let output_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
            .plain_line;
        assert!(output_line.contains("↓ 200 tokens"));

        model.add_stream_activity_input_tokens_at(100, started_at + Duration::from_millis(140));
        let input_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(260))
            .plain_line;
        assert!(input_line.contains("↑ 300 tokens"));
        assert!(!input_line.contains("↓ 200 tokens"));
    }

    #[test]
    fn stream_activity_thinking_segment_renders_between_timer_and_tokens() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        model.set_stream_activity_thinking(true);
        model.set_stream_activity_output_tokens_at(12, started_at);

        let thinking_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
            .plain_line;
        assert!(thinking_line.contains("(0s • thinking • ↓ 12 tokens"));

        model.set_stream_activity_thinking(false);
        let content_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(140))
            .plain_line;
        assert!(!content_line.contains("thinking"));
        assert!(content_line.contains("(0s • ↓ 12 tokens"));
    }

    #[test]
    fn stream_activity_token_indicator_compacts_thousands_to_k_unit() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        model.set_stream_activity_output_tokens_at(999, started_at);
        let under_k_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(120))
            .plain_line;
        assert!(under_k_line.contains("↓ 999 tokens"));

        model.set_stream_activity_output_tokens_at(1_200, started_at + Duration::from_millis(140));
        let k_line = model
            .current_stream_activity_render_result_at(started_at + Duration::from_millis(260))
            .plain_line;
        assert!(k_line.contains("↓ 1.2k tokens"));
        assert!(!k_line.contains("1200 tokens"));
    }

    #[test]
    fn stream_activity_token_indicator_uses_fast_tick_until_target_or_stale() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let started_at = model.stream_activity.as_ref().unwrap().started_at;
        model.set_stream_activity_output_tokens_at(36, started_at);

        assert_eq!(
            model.stream_activity_frame_interval_at(started_at + Duration::from_millis(33)),
            Some(Duration::from_millis(33))
        );
        assert_eq!(
            model.stream_activity_frame_interval_at(started_at + Duration::from_millis(130)),
            Some(Duration::from_millis(80)),
            "token tick should stop once the displayed value catches the target"
        );

        model.set_stream_activity_output_tokens_at(72, started_at + Duration::from_millis(200));
        assert_eq!(
            model.stream_activity_frame_interval_at(started_at + Duration::from_millis(600)),
            Some(Duration::from_millis(80)),
            "stale token snapshots should not keep the fast tick alive"
        );
    }

    #[test]
    fn document_layout_rebuilds_when_stream_activity_tick_changes() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.sync_transcript_render();
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_stream_activity_with_header("Working");

        let initial = model.build_document_layout();
        assert!(
            initial.tail.text_lines[0].contains("Working (0s"),
            "activity should include the current elapsed segment"
        );

        model.stream_activity.as_mut().unwrap().started_at -= std::time::Duration::from_secs(2);
        let updated = model.build_document_layout();

        assert!(
            updated.tail.text_lines[0].contains("Working (2s"),
            "outer document layout cache must not hide updated activity text"
        );
    }
}
