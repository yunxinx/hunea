use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthStr;

use super::{Model, document::ViewportState};

pub(crate) const EXIT_CONFIRMATION_PROMPT: &str = "Press again to exit";
pub(crate) const EXIT_CONFIRMATION_WINDOW: Duration = Duration::from_secs(1);

impl Model {
    pub(crate) fn current_status_notice_text(&self) -> &str {
        &self.notice_state.status_text
    }

    pub(crate) fn show_exit_confirmation(&mut self) {
        self.notice_state.exit_confirmation_deadline =
            Some(Instant::now() + EXIT_CONFIRMATION_WINDOW);
        self.show_status_notice(EXIT_CONFIRMATION_PROMPT);
    }

    pub(crate) fn cancel_exit_confirmation(&mut self) {
        if self.notice_state.exit_confirmation_deadline.is_none() {
            return;
        }

        self.notice_state.exit_confirmation_deadline = None;
        if self.notice_state.status_text != EXIT_CONFIRMATION_PROMPT {
            return;
        }

        self.notice_state.status_deadline = None;
        self.set_status_notice_text(String::new());
    }

    pub(crate) fn dismiss_status_notice(&mut self, token: usize) {
        if self.notice_state.status_text.is_empty() || token != self.notice_state.status_token {
            return;
        }

        self.clear_status_notice();
    }

    pub(crate) fn clear_status_notice(&mut self) {
        self.notice_state.exit_confirmation_deadline = None;
        self.notice_state.status_deadline = None;
        self.set_status_notice_text(String::new());
    }

    pub(crate) fn exit_confirmation_active(&self, now: Instant) -> bool {
        self.notice_state
            .exit_confirmation_deadline
            .is_some_and(|deadline| now <= deadline)
    }

    pub(crate) fn show_transient_status_notice(&mut self, text: &str) {
        self.notice_state.exit_confirmation_deadline = None;
        self.show_status_notice(text);
    }

    pub(crate) fn current_status_notice_render_result(
        &self,
    ) -> super::status_line::StatusLineRenderResult {
        let width = if self.width == 0 {
            super::transcript::DEFAULT_RENDER_WIDTH
        } else {
            usize::from(self.width)
        };
        let content_width = width.saturating_sub(super::status_line::STATUS_LINE_INSET_WIDTH);
        let text = super::status_line::truncate_display_width(
            self.current_status_notice_text(),
            content_width,
        );
        if text.is_empty() {
            return super::status_line::StatusLineRenderResult::default();
        }

        let plain_line = format!(
            "{}{}",
            " ".repeat(super::status_line::STATUS_LINE_INSET_WIDTH),
            text
        );
        super::status_line::StatusLineRenderResult {
            line: Some(ratatui::text::Line::styled(
                plain_line.clone(),
                super::theme::tertiary_text_style(self.palette)
                    .bold()
                    .italic(),
            )),
            plain_line,
            selectable: super::selection::SelectableLineRange::new(
                super::status_line::STATUS_LINE_INSET_WIDTH,
                super::status_line::STATUS_LINE_INSET_WIDTH + text.width(),
            ),
            has_content: true,
            gap_before: super::status_line::status_line_gap_before(self.style_mode),
        }
    }

    pub(crate) fn sync_after_bottom_status_slot_change(
        &mut self,
        preserved_viewport_state: Option<ViewportState>,
    ) {
        self.sync_composer_height();
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.document_runtime.manual_scroll {
            if let Some(state) = preserved_viewport_state.as_ref() {
                self.sync_document_viewport_for_viewport_state(state);
            } else {
                self.sync_document_viewport_preserving_position();
            }
            self.complete_manual_document_scroll_if_restored();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    fn show_status_notice(&mut self, text: &str) {
        self.notice_state.status_token += 1;
        self.notice_state.status_deadline = Some(Instant::now() + EXIT_CONFIRMATION_WINDOW);
        self.set_status_notice_text(text.to_string());
    }

    fn set_status_notice_text(&mut self, text: String) {
        if self.notice_state.status_text == text {
            return;
        }

        self.maybe_clear_selection_for_bottom_status_slot_change();
        self.maybe_clear_pending_composer_cursor_click_for_bottom_status_slot_change();
        let preserved_viewport_state = if self.document_runtime.manual_scroll {
            Some(self.current_document_viewport_state())
        } else {
            None
        };

        self.notice_state.status_text = text;
        self.bump_status_line_revision();
        self.sync_after_bottom_status_slot_change(preserved_viewport_state);
    }
}
