use super::*;
use std::mem;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToastSeverity {
    Info,
    Error,
}

impl ToastSeverity {
    const fn hold_duration(self) -> Duration {
        match self {
            Self::Info => Duration::from_secs(2),
            Self::Error => Duration::from_secs(3),
        }
    }

    pub(super) fn border_color(self, palette: TerminalPalette) -> Color {
        match self {
            Self::Info => palette.accent,
            Self::Error => palette.system_error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToastNotice {
    pub(super) severity: ToastSeverity,
    pub(super) text: String,
}

impl ToastNotice {
    pub(super) fn new(severity: ToastSeverity, text: impl Into<String>) -> Self {
        Self {
            severity,
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ToastState {
    phase: ToastPhase,
    pending: Option<ToastNotice>,
    visible_token: usize,
}

#[derive(Debug, Clone)]
enum ToastPhase {
    Idle,
    Entering(ToastAnimation),
    Visible {
        notice: ToastNotice,
        until: Instant,
        token: usize,
    },
    Exiting(ToastAnimation),
}

#[derive(Debug, Clone)]
pub(super) struct ToastAnimation {
    pub(super) notice: ToastNotice,
    pub(super) kind: ToastAnimationKind,
    pub(super) started_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastAnimationKind {
    Enter,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ToastAnimationFrame {
    pub(super) erased_columns: u16,
    pub(super) visible_columns: u16,
    pub(super) is_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastAnimationCompletion {
    Enter,
    Exit,
}

impl Default for ToastState {
    fn default() -> Self {
        Self {
            phase: ToastPhase::Idle,
            pending: None,
            visible_token: 0,
        }
    }
}

impl ToastState {
    pub(super) fn show(&mut self, severity: ToastSeverity, text: impl Into<String>) {
        let notice = ToastNotice::new(severity, text);
        if matches!(self.phase, ToastPhase::Idle) {
            self.start_entering(notice);
            return;
        }

        self.pending = Some(notice);
        if !matches!(self.phase, ToastPhase::Exiting(_)) {
            self.start_exiting_active_notice();
        }
    }

    pub(super) fn next_timeout_deadline(&self) -> Option<Instant> {
        match self.phase {
            ToastPhase::Visible { until, .. } => Some(until),
            ToastPhase::Idle | ToastPhase::Entering(_) | ToastPhase::Exiting(_) => None,
        }
    }

    pub(super) fn timeout_token(&self) -> usize {
        match self.phase {
            ToastPhase::Visible { token, .. } => token,
            ToastPhase::Idle | ToastPhase::Entering(_) | ToastPhase::Exiting(_) => {
                self.visible_token
            }
        }
    }

    pub(super) fn handle_visible_timeout(&mut self, token: usize) {
        match mem::replace(&mut self.phase, ToastPhase::Idle) {
            ToastPhase::Visible {
                notice,
                token: current_token,
                ..
            } if current_token == token => {
                self.start_exiting(notice);
            }
            phase => {
                self.phase = phase;
            }
        }
    }

    pub(super) fn frame_interval(&self) -> Option<Duration> {
        matches!(self.phase, ToastPhase::Entering(_) | ToastPhase::Exiting(_))
            .then_some(TOAST_FRAME_INTERVAL)
    }

    pub(super) fn advance_at(&mut self, now: Instant) {
        let completion = match &mut self.phase {
            ToastPhase::Entering(animation) => animation
                .advance_at(now)
                .then_some(ToastAnimationCompletion::Enter),
            ToastPhase::Exiting(animation) => animation
                .advance_at(now)
                .then_some(ToastAnimationCompletion::Exit),
            ToastPhase::Idle | ToastPhase::Visible { .. } => None,
        };

        match completion {
            Some(ToastAnimationCompletion::Enter) => self.finish_enter(now),
            Some(ToastAnimationCompletion::Exit) => self.finish_exit(),
            None => {}
        }
    }

    pub(super) fn render_at(
        &self,
        now: Instant,
        area: Rect,
        buffer: &mut Buffer,
        palette: TerminalPalette,
    ) {
        let Some(active_text) = self.active_notice().map(|notice| notice.text.as_str()) else {
            return;
        };
        let Some(toast_area) = toast_rect(area, active_text) else {
            return;
        };

        match &self.phase {
            ToastPhase::Entering(animation) => {
                let frame = animation.frame_at(now, toast_area.width);
                render_toast_transition(
                    &animation.notice,
                    toast_area,
                    buffer,
                    palette,
                    animation.kind,
                    frame,
                    None,
                );
            }
            ToastPhase::Exiting(animation) => {
                let underlay = ToastUnderlaySnapshot::capture(buffer, toast_area);
                let frame = animation.frame_at(now, toast_area.width);
                render_toast_transition(
                    &animation.notice,
                    toast_area,
                    buffer,
                    palette,
                    animation.kind,
                    frame,
                    Some(&underlay),
                );
            }
            ToastPhase::Visible { notice, .. } => {
                render_toast_notice(notice, toast_area, buffer, palette);
            }
            ToastPhase::Idle => {}
        }
    }

    fn start_entering(&mut self, notice: ToastNotice) {
        self.phase = ToastPhase::Entering(ToastAnimation::enter(notice));
    }

    fn start_exiting(&mut self, notice: ToastNotice) {
        self.phase = ToastPhase::Exiting(ToastAnimation::exit(notice));
    }

    fn start_exiting_active_notice(&mut self) {
        match mem::replace(&mut self.phase, ToastPhase::Idle) {
            ToastPhase::Entering(animation) => self.start_exiting(animation.notice),
            ToastPhase::Visible { notice, .. } => self.start_exiting(notice),
            phase @ (ToastPhase::Idle | ToastPhase::Exiting(_)) => {
                self.phase = phase;
            }
        }
    }

    fn finish_enter(&mut self, now: Instant) {
        match mem::replace(&mut self.phase, ToastPhase::Idle) {
            ToastPhase::Entering(animation) => {
                let notice = animation.notice;
                self.visible_token = self.visible_token.saturating_add(1);
                self.phase = ToastPhase::Visible {
                    until: now + notice.severity.hold_duration(),
                    notice,
                    token: self.visible_token,
                };
            }
            phase => {
                self.phase = phase;
            }
        }
    }

    fn finish_exit(&mut self) {
        match mem::replace(&mut self.phase, ToastPhase::Idle) {
            ToastPhase::Exiting(_) => {
                if let Some(next_notice) = self.pending.take() {
                    self.start_entering(next_notice);
                }
            }
            phase => {
                self.phase = phase;
            }
        }
    }

    fn active_notice(&self) -> Option<&ToastNotice> {
        match &self.phase {
            ToastPhase::Idle => None,
            ToastPhase::Entering(animation) | ToastPhase::Exiting(animation) => {
                Some(&animation.notice)
            }
            ToastPhase::Visible { notice, .. } => Some(notice),
        }
    }

    #[cfg(test)]
    pub(super) fn active_text(&self) -> Option<&str> {
        self.active_notice().map(|notice| notice.text.as_str())
    }

    #[cfg(test)]
    pub(super) fn pending_text(&self) -> Option<&str> {
        self.pending.as_ref().map(|notice| notice.text.as_str())
    }

    #[cfg(test)]
    pub(super) const fn is_entering(&self) -> bool {
        matches!(self.phase, ToastPhase::Entering(_))
    }

    #[cfg(test)]
    pub(super) const fn is_exiting(&self) -> bool {
        matches!(self.phase, ToastPhase::Exiting(_))
    }
}
