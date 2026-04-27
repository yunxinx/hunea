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
    status_line::{
        STATUS_LINE_INSET_WIDTH, StatusLineRenderResult, truncate_display_width_with_ellipsis,
    },
    theme::{TerminalPalette, secondary_text_style},
    transcript::DEFAULT_RENDER_WIDTH,
};

const ACP_ACTIVITY_FRAME_INTERVAL: Duration = Duration::from_millis(80);
const ACP_ACTIVITY_GLYPH: &str = "•";

/// `AcpActivityState` 保存 ACP turn 正在运行时显示在输入框上方的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AcpActivityState {
    started_at: Instant,
    header: String,
    interrupt_hint: Option<String>,
}

impl Model {
    pub(crate) fn show_acp_activity(&mut self, text: impl Into<String>) {
        let text = text.into();
        if text.trim().is_empty() {
            return;
        }

        let header = self.status_phrase_selector.next_phrase();
        self.show_acp_activity_with_header(header);
    }

    pub(crate) fn show_acp_activity_with_header(&mut self, header: impl Into<String>) {
        let header = header.into().trim().to_string();
        if header.is_empty() {
            return;
        }

        self.acp_activity = Some(AcpActivityState {
            started_at: Instant::now(),
            header,
            interrupt_hint: self.current_acp_activity_interrupt_hint(),
        });
        self.reset_chat_interrupt_esc_count();
        self.bump_status_line_revision();
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
        }
    }

    fn current_acp_activity_interrupt_hint(&self) -> Option<String> {
        if !self.show_esc_interrupt_hint {
            return None;
        }

        Some(match self.esc_interrupt_presses {
            1 => "esc to interrupt".to_string(),
            presses => format!("esc {presses}x to interrupt"),
        })
    }

    pub(crate) fn clear_acp_activity(&mut self) {
        if self.acp_activity.is_none() {
            return;
        }

        self.acp_activity = None;
        self.reset_chat_interrupt_esc_count();
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
        let (text, spans) = render_activity_content(activity, self.palette, now, content_width);
        if text.is_empty() {
            return StatusLineRenderResult::default();
        }

        let plain_line = format!("{}{}", " ".repeat(STATUS_LINE_INSET_WIDTH), text);
        let mut line_spans = Vec::with_capacity(spans.len() + 1);
        line_spans.push(Span::raw(" ".repeat(STATUS_LINE_INSET_WIDTH)));
        line_spans.extend(spans);

        StatusLineRenderResult {
            line: Some(Line::from(line_spans)),
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

    fn elapsed_segment_at(&self, now: Instant) -> String {
        let elapsed = self.elapsed_text_at(now);
        match self.interrupt_hint.as_deref() {
            Some(hint) => format!("({elapsed} • {hint})"),
            None => format!("({elapsed})"),
        }
    }

    fn frame_index_at(&self, now: Instant) -> usize {
        let interval_ms = ACP_ACTIVITY_FRAME_INTERVAL.as_millis().max(1);
        let tick = self.elapsed_at(now).as_millis() / interval_ms;
        tick as usize
    }
}

fn render_activity_content(
    activity: &AcpActivityState,
    palette: TerminalPalette,
    now: Instant,
    content_width: usize,
) -> (String, Vec<Span<'static>>) {
    let elapsed_text = activity.elapsed_segment_at(now);
    let text = format!(
        "{ACP_ACTIVITY_GLYPH} {} {elapsed_text}",
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
    activity: &AcpActivityState,
    palette: TerminalPalette,
    now: Instant,
    elapsed_text: String,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    spans.extend(shimmer_spans_at(
        ACP_ACTIVITY_GLYPH,
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

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;
    use crate::frontend::tui::{HeroOptions, theme::default_palette};

    #[test]
    fn acp_activity_tail_cache_key_changes_when_elapsed_text_changes() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_acp_activity_with_header("Working");

        let started_at = model.acp_activity.as_ref().unwrap().started_at;
        let initial_key = model.acp_activity_frame_key(started_at);
        let later_key =
            model.acp_activity_frame_key(started_at + std::time::Duration::from_millis(1_200));

        assert_ne!(
            initial_key, later_key,
            "activity cache key must change when the visible elapsed timer changes"
        );
    }

    #[test]
    fn acp_activity_line_uses_shimmer_spans_without_changing_plain_text() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_acp_activity_with_header("Working");

        let started_at = model.acp_activity.as_ref().unwrap().started_at;
        let first = model.current_acp_activity_render_result_at(started_at);
        let second = model.current_acp_activity_render_result_at(
            started_at + std::time::Duration::from_millis(900),
        );
        let first_line = first.line.expect("activity line should render");
        let second_line = second.line.expect("activity line should render");

        assert_eq!(first.plain_line, "  • Working (0s • esc 2x to interrupt)");
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
    fn document_layout_rebuilds_when_acp_activity_tick_changes() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.sync_transcript_render();
        model.set_window(50, 6);
        model.set_palette(default_palette(), true);
        model.show_acp_activity_with_header("Working");

        let initial = model.build_document_layout();
        assert!(
            initial.tail.text_lines[0].contains("Working (0s"),
            "activity should include the current elapsed segment"
        );

        model.acp_activity.as_mut().unwrap().started_at -= std::time::Duration::from_secs(2);
        let updated = model.build_document_layout();

        assert!(
            updated.tail.text_lines[0].contains("Working (2s"),
            "outer document layout cache must not hide updated activity text"
        );
    }
}
