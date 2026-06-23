use super::*;

impl Model {
    /// 显示一个上层 toast notice。
    pub(crate) fn show_toast(&mut self, severity: ToastSeverity, text: impl Into<String>) {
        self.toast_state.show(severity, text);
    }

    pub(crate) fn toast_timeout_deadline(&self) -> Option<Instant> {
        self.toast_state.next_timeout_deadline()
    }

    pub(crate) fn toast_timeout_token(&self) -> usize {
        self.toast_state.timeout_token()
    }

    pub(crate) fn handle_toast_timeout(&mut self, token: usize) {
        self.toast_state.handle_visible_timeout(token);
    }

    pub(crate) fn toast_frame_interval(&self) -> Option<Duration> {
        self.toast_state.frame_interval()
    }

    pub(crate) fn advance_toast_at(&mut self, now: Instant) {
        self.toast_state.advance_at(now);
    }

    pub(crate) fn render_toast(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let now = frame.now();
        self.toast_state
            .render_at(now, area, frame.buffer_mut(), self.palette);
    }

    #[cfg(test)]
    pub(crate) fn active_toast_text_for_test(&self) -> Option<&str> {
        self.toast_state.active_text()
    }
}
