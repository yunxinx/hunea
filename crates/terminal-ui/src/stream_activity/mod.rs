//! 运行中 stream activity 的状态与渲染。

mod render;
mod state;

use std::time::{Duration, Instant};

use ratatui::text::Line;

use self::{
    render::render_activity_content,
    state::{STREAM_ACTIVITY_ELAPSED_TICK_INTERVAL, should_append_work_duration_summary},
};
pub(crate) use state::{StreamActivityFrameKey, StreamActivityState, format_elapsed_compact};

use super::{
    Model, display_width::display_width, frame_time::next_animation_frame_deadline,
    selection::SelectableLineRange, status_line::StatusLineRenderResult,
    tool_result::TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL, transcript::DEFAULT_RENDER_WIDTH,
};

const STREAM_ACTIVITY_GLYPH: &str = "•";

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
        self.bump_stream_activity_revision();
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
            self.bump_stream_activity_revision();
            self.sync_composer_height();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
            }
            return;
        };
        if !activity.enter_retry(header, now) {
            return;
        }

        self.bump_stream_activity_revision();
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

        self.bump_stream_activity_revision();
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
        self.bump_stream_activity_revision();
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
        self.bump_stream_activity_revision();
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

        self.bump_stream_activity_revision();
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
        self.bump_stream_activity_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn record_stream_activity_output_tokens_at(&mut self, total_tokens: usize, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.record_output_tokens(total_tokens, now);
        self.bump_stream_activity_revision();
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
        self.bump_stream_activity_revision();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn record_stream_activity_input_tokens_at(&mut self, total_tokens: usize, now: Instant) {
        let Some(activity) = self.stream_activity.as_mut() else {
            return;
        };
        activity.record_input_tokens(total_tokens, now);
        self.bump_stream_activity_revision();
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
        let render_now = activity.active_now(now);
        let (text, spans) =
            render_activity_content(activity, self.palette, render_now, width, self.motion_mode);
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

    fn bump_stream_activity_revision(&mut self) {
        self.stream_activity_revision = self.stream_activity_revision.saturating_add(1);
    }

    pub(crate) fn stream_activity_frame_key(&self, now: Instant) -> StreamActivityFrameKey {
        if !self.motion_mode.allows_animation() {
            let frame_index = self
                .stream_activity
                .as_ref()
                .map(|activity| activity.elapsed_at(activity.active_now(now)).as_secs() as usize)
                .unwrap_or(0);
            return StreamActivityFrameKey {
                revision: self.stream_activity_revision,
                frame_index,
            };
        }
        let frame_index = self
            .stream_activity
            .as_ref()
            .map(|activity| activity.frame_index_at(activity.active_now(now)))
            .unwrap_or(0);
        StreamActivityFrameKey {
            revision: self.stream_activity_revision,
            frame_index,
        }
    }

    pub(crate) fn stream_activity_next_frame_deadline_at(&self, now: Instant) -> Option<Instant> {
        let activity = self
            .stream_activity
            .as_ref()
            .filter(|activity| !activity.is_paused())?;
        if !self.motion_mode.allows_animation() {
            return next_animation_frame_deadline(
                activity.started_at,
                now,
                STREAM_ACTIVITY_ELAPSED_TICK_INTERVAL,
            );
        }
        next_animation_frame_deadline(activity.started_at, now, activity.frame_interval_at(now))
    }

    pub(crate) fn tool_activity_frame_key(&self, now: Instant) -> usize {
        if !self.motion_mode.allows_animation() {
            return usize::from(self.transcript.active_tool_activity_started_at().is_some());
        }
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
        if !self.motion_mode.allows_animation() {
            return None;
        }
        let started_at = self.transcript.active_tool_activity_started_at()?;
        next_animation_frame_deadline(started_at, now, TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL)
    }
}

#[cfg(test)]
use render::activity_glyph_span_at;
#[cfg(test)]
use state::STREAM_ACTIVITY_FRAME_INTERVAL;

#[cfg(test)]
mod tests;
