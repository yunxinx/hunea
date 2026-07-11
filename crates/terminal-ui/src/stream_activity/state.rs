use std::time::{Duration, Instant};

pub(super) const STREAM_ACTIVITY_FRAME_INTERVAL: Duration = Duration::from_millis(80);
pub(super) const STREAM_ACTIVITY_ELAPSED_TICK_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const STREAM_ACTIVITY_TOKEN_TICK_INTERVAL: Duration = Duration::from_millis(33);
const TOKEN_TWEEN_DURATION: Duration = Duration::from_millis(120);
const TOKEN_STALE_THRESHOLD: Duration = Duration::from_millis(360);
pub(super) const WORK_DURATION_SUMMARY_MIN_ELAPSED_SECS: u64 = 30;

/// `StreamActivityState` 保存一次模型 turn 运行中显示在输入框上方的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StreamActivityState {
    pub(super) started_at: Instant,
    pub(super) header: String,
    pub(super) retry_header: Option<String>,
    pub(super) interrupt_hint: Option<String>,
    pub(super) output_tokens: Option<ActivityTokenProgress>,
    pub(super) is_thinking: bool,
    pub(super) paused_at: Option<Instant>,
}

/// `StreamActivityFrameKey` 同时描述 activity 内容状态与当前动画帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct StreamActivityFrameKey {
    pub(super) revision: usize,
    pub(super) frame_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActivityTokenProgress {
    previous_display: usize,
    pub(super) target: usize,
    output_total: usize,
    input_total: usize,
    pub(super) direction: ActivityTokenDirection,
    updated_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivityTokenDirection {
    Down,
    Up,
}

impl StreamActivityState {
    pub(super) fn display_header(&self) -> &str {
        self.retry_header.as_deref().unwrap_or(&self.header)
    }

    pub(super) fn has_retry_header(&self) -> bool {
        self.retry_header.is_some()
    }

    pub(super) fn enter_retry(&mut self, header: String, now: Instant) -> bool {
        let mut changed = self.set_retry_header(header);
        changed |= self.clear_attempt_progress();
        if self.paused_at.is_none() {
            self.pause_at(now);
            changed = true;
        }
        changed
    }

    pub(super) fn exit_retry(&mut self, now: Instant) -> bool {
        if self.retry_header.is_none() {
            return false;
        }

        self.retry_header = None;
        let _ = self.resume_at(now);
        true
    }

    pub(super) fn set_retry_header(&mut self, header: String) -> bool {
        if self.retry_header.as_deref() == Some(header.as_str()) {
            return false;
        }

        self.retry_header = Some(header);
        true
    }

    pub(super) fn clear_attempt_progress(&mut self) -> bool {
        let had_progress = self.is_thinking || self.output_tokens.is_some();
        self.is_thinking = false;
        self.output_tokens = None;
        had_progress
    }

    pub(super) fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub(super) fn active_now(&self, now: Instant) -> Instant {
        self.paused_at.unwrap_or(now)
    }

    pub(super) fn pause_at(&mut self, now: Instant) {
        self.paused_at = Some(now);
    }

    pub(super) fn resume_at(&mut self, now: Instant) -> bool {
        let Some(paused_at) = self.paused_at.take() else {
            return false;
        };
        let paused_for = now.saturating_duration_since(paused_at);
        self.shift_activity_clock(paused_for);
        true
    }

    pub(super) fn shift_activity_clock(&mut self, offset: Duration) {
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

    pub(super) fn elapsed_at(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started_at)
    }

    pub(super) fn elapsed_text_at(&self, now: Instant) -> String {
        format_elapsed_compact(self.elapsed_at(now).as_secs())
    }

    pub(super) fn elapsed_segment_at(&self, now: Instant) -> String {
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

    pub(super) fn reduced_segment_at(&self, now: Instant) -> String {
        let mut segments = vec![self.elapsed_text_at(now)];
        if self.is_thinking {
            segments.push("thinking".to_string());
        }
        if let Some(progress) = self.output_tokens.as_ref()
            && progress.target > 0
        {
            segments.push(format!(
                "{} {} tokens",
                progress.direction.glyph(),
                format_token_count(progress.target)
            ));
        }
        if let Some(hint) = self.interrupt_hint.as_deref() {
            segments.push(hint.to_string());
        }
        format!("({})", segments.join(" • "))
    }

    pub(super) fn frame_index_at(&self, now: Instant) -> usize {
        let interval_ms = self.frame_interval_at(now).as_millis().max(1);
        let tick = self.elapsed_at(now).as_millis() / interval_ms;
        let token_display = self.output_tokens_display_at(now);
        (tick as usize)
            .saturating_mul(1_000_003)
            .saturating_add(token_display)
    }

    pub(super) fn output_tokens_display_at(&self, now: Instant) -> usize {
        self.output_tokens
            .as_ref()
            .map(|progress| progress.display_at(now))
            .unwrap_or(0)
    }

    pub(super) fn token_segment_at(&self, now: Instant) -> Option<String> {
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

    pub(super) fn frame_interval_at(&self, now: Instant) -> Duration {
        if self
            .output_tokens
            .as_ref()
            .is_some_and(|progress| progress.needs_fast_tick_at(now))
        {
            return STREAM_ACTIVITY_TOKEN_TICK_INTERVAL;
        }
        STREAM_ACTIVITY_FRAME_INTERVAL
    }

    pub(super) fn record_output_tokens(&mut self, total_tokens: usize, now: Instant) {
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
    pub(super) fn add_input_tokens(&mut self, token_delta: usize, now: Instant) {
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

    pub(super) fn record_input_tokens(&mut self, total_tokens: usize, now: Instant) {
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

    pub(super) fn replace_token_progress(
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
    pub(super) fn shift_clock(&mut self, offset: Duration) {
        if let Some(updated_at) = self.updated_at.checked_add(offset) {
            self.updated_at = updated_at;
        }
    }

    pub(super) fn display_at(&self, now: Instant) -> usize {
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

    pub(super) fn needs_fast_tick_at(&self, now: Instant) -> bool {
        self.display_at(now) < self.target
            && now.saturating_duration_since(self.updated_at) <= TOKEN_STALE_THRESHOLD
    }
}

impl ActivityTokenDirection {
    pub(super) fn glyph(self) -> &'static str {
        match self {
            Self::Down => "↓",
            Self::Up => "↑",
        }
    }
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

pub(super) fn should_append_work_duration_summary(duration: Duration) -> bool {
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
