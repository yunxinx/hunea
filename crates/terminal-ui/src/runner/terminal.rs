use std::io;

use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;

use crate::terminal_lifecycle::TerminalLifecycleGuard;

use super::{event_pipeline::TerminalWaitPlan, terminal_surface::TerminalSurface};

pub(super) type TuiTerminal = TerminalSurface<CrosstermBackend<io::Stdout>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalMouseModePreference {
    Capture,
    NativeWithAlternateScroll,
    CaptureWithAlternateScroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalMouseMode {
    has_mouse_capture: bool,
    has_alternate_scroll: bool,
}

impl TerminalMouseMode {
    pub(super) const fn from_preference(preference: TerminalMouseModePreference) -> Self {
        match preference {
            TerminalMouseModePreference::Capture => Self {
                has_mouse_capture: true,
                has_alternate_scroll: false,
            },
            TerminalMouseModePreference::NativeWithAlternateScroll => Self {
                has_mouse_capture: false,
                has_alternate_scroll: true,
            },
            TerminalMouseModePreference::CaptureWithAlternateScroll => Self {
                has_mouse_capture: true,
                has_alternate_scroll: true,
            },
        }
    }

    pub(super) const fn for_mouse_capture(has_mouse_capture: bool) -> Self {
        if has_mouse_capture {
            Self::from_preference(TerminalMouseModePreference::Capture)
        } else {
            Self::from_preference(TerminalMouseModePreference::NativeWithAlternateScroll)
        }
    }
}

pub(super) fn wait_for_terminal_event(wait_plan: TerminalWaitPlan) -> io::Result<Option<Event>> {
    match wait_plan {
        TerminalWaitPlan::Block => event::read().map(Some),
        TerminalWaitPlan::Poll { duration, .. } => {
            if event::poll(duration)? {
                event::read().map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

pub(super) struct TerminalSession {
    lifecycle: TerminalLifecycleGuard,
}

impl TerminalSession {
    pub(super) fn enter() -> io::Result<(TuiTerminal, Self)> {
        let mut lifecycle = TerminalLifecycleGuard::default();
        let mut stdout = io::stdout();
        lifecycle.activate_main(&mut stdout)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = TerminalSurface::new(backend)?;
        lifecycle.hide_cursor_with(|| terminal.hide_cursor())?;
        Ok((terminal, Self { lifecycle }))
    }

    pub(super) fn apply_mouse_mode(
        &mut self,
        terminal: &mut TuiTerminal,
        mode: TerminalMouseMode,
    ) -> io::Result<()> {
        self.lifecycle.apply_mouse_mode(
            terminal.backend_mut(),
            mode.has_mouse_capture,
            mode.has_alternate_scroll,
        )
    }

    pub(super) fn suspend(&mut self, terminal: &mut TuiTerminal) -> io::Result<()> {
        let mut first_error = self
            .lifecycle
            .show_cursor_with(|| terminal.show_cursor())
            .err();
        if let Err(error) = self.lifecycle.restore_modes(terminal.backend_mut())
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        finish_terminal_transition(first_error)
    }

    pub(super) fn resume(&mut self, terminal: &mut TuiTerminal) -> io::Result<()> {
        self.lifecycle.activate_main(terminal.backend_mut())?;
        if let Err(error) = self.lifecycle.hide_cursor_with(|| terminal.hide_cursor()) {
            let _ = self.lifecycle.restore_all(terminal.backend_mut());
            return Err(error);
        }
        if let Err(error) = terminal.clear() {
            let _ = self.lifecycle.restore_all(terminal.backend_mut());
            return Err(error);
        }
        Ok(())
    }
}

fn finish_terminal_transition(first_error: Option<io::Error>) -> io::Result<()> {
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalMouseMode;

    #[test]
    fn overlay_mouse_mode_uses_alternate_scroll_without_mouse_capture() {
        assert_eq!(
            TerminalMouseMode::for_mouse_capture(true),
            TerminalMouseMode {
                has_mouse_capture: true,
                has_alternate_scroll: false,
            }
        );
        assert_eq!(
            TerminalMouseMode::for_mouse_capture(false),
            TerminalMouseMode {
                has_mouse_capture: false,
                has_alternate_scroll: true,
            }
        );
        assert_eq!(
            TerminalMouseMode::from_preference(
                super::TerminalMouseModePreference::CaptureWithAlternateScroll
            ),
            TerminalMouseMode {
                has_mouse_capture: true,
                has_alternate_scroll: true,
            }
        );
    }
}
