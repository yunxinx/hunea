use std::time::{Duration, Instant};

use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use super::{
    Model,
    selection::SelectableLineRange,
    status_line::{
        STATUS_LINE_INSET_WIDTH, StatusLineRenderResult, truncate_display_width_with_ellipsis,
    },
    theme::tertiary_text_style,
    transcript::DEFAULT_RENDER_WIDTH,
};

const ACP_ACTIVITY_FRAME_INTERVAL: Duration = Duration::from_millis(600);
const ACP_ACTIVITY_FRAMES: [&str; 2] = ["•", "◦"];

/// `AcpActivityState` 保存 ACP turn 正在运行时显示在输入框上方的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcpActivityState {
    started_at: Instant,
}

impl Model {
    pub(crate) fn show_acp_activity(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        self.acp_activity = Some(AcpActivityState {
            started_at: Instant::now(),
        });
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn clear_acp_activity(&mut self) {
        if self.acp_activity.is_none() {
            return;
        }

        self.acp_activity = None;
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    pub(crate) fn current_acp_activity_render_result(&self) -> StatusLineRenderResult {
        self.current_acp_activity_render_result_at(Instant::now())
    }

    pub(crate) fn current_acp_activity_render_result_at(
        &self,
        now: Instant,
    ) -> StatusLineRenderResult {
        let Some(activity) = self.acp_activity.as_ref() else {
            return StatusLineRenderResult::default();
        };

        let width = if self.width == 0 {
            DEFAULT_RENDER_WIDTH
        } else {
            usize::from(self.width)
        };
        let content_width = width.saturating_sub(STATUS_LINE_INSET_WIDTH);
        let text = truncate_display_width_with_ellipsis(
            &format!(
                "{} Working ({})",
                activity.status_glyph_at(now),
                activity.elapsed_text_at(now)
            ),
            content_width,
        );
        if text.is_empty() {
            return StatusLineRenderResult::default();
        }

        let plain_line = format!("{}{}", " ".repeat(STATUS_LINE_INSET_WIDTH), text);
        StatusLineRenderResult {
            line: Some(Line::styled(
                plain_line.clone(),
                tertiary_text_style(self.palette).italic(),
            )),
            plain_line,
            selectable: SelectableLineRange::new(
                STATUS_LINE_INSET_WIDTH,
                STATUS_LINE_INSET_WIDTH + text.width(),
            ),
            has_content: true,
            gap_before: 0,
        }
    }

    pub(crate) fn acp_activity_frame_key(&self, now: Instant) -> usize {
        self.acp_activity
            .as_ref()
            .map(|activity| activity.frame_index_at(now))
            .unwrap_or(0)
    }

    pub(crate) fn acp_activity_frame_interval(&self) -> Option<Duration> {
        self.acp_activity
            .is_some()
            .then_some(ACP_ACTIVITY_FRAME_INTERVAL)
    }
}

impl AcpActivityState {
    fn elapsed_at(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started_at)
    }

    fn elapsed_text_at(&self, now: Instant) -> String {
        format_elapsed_compact(self.elapsed_at(now).as_secs())
    }

    fn frame_index_at(&self, now: Instant) -> usize {
        let interval_ms = ACP_ACTIVITY_FRAME_INTERVAL.as_millis().max(1);
        let tick = self.elapsed_at(now).as_millis() / interval_ms;
        (tick as usize) % ACP_ACTIVITY_FRAMES.len()
    }

    fn status_glyph_at(&self, now: Instant) -> &'static str {
        ACP_ACTIVITY_FRAMES[self.frame_index_at(now)]
    }
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
