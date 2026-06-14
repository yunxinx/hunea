use std::{fmt, io};

use crossterm::{
    Command,
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::backend::CrosstermBackend;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnableAlternateScroll;

impl Command for EnableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007h")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute EnableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableAlternateScroll;

impl Command for DisableAlternateScroll {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[?1007l")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other(
            "tried to execute DisableAlternateScroll using WinAPI; use ANSI instead",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
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

pub(super) fn apply_mouse_mode(
    terminal: &mut TuiTerminal,
    mode: TerminalMouseMode,
) -> io::Result<()> {
    match (mode.has_mouse_capture, mode.has_alternate_scroll) {
        (true, false) => execute!(
            terminal.backend_mut(),
            DisableAlternateScroll,
            EnableMouseCapture
        ),
        (false, true) => execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            EnableAlternateScroll
        ),
        (true, true) => execute!(
            terminal.backend_mut(),
            EnableAlternateScroll,
            EnableMouseCapture
        ),
        (false, false) => execute!(
            terminal.backend_mut(),
            DisableMouseCapture,
            DisableAlternateScroll
        ),
    }
}

pub(super) struct TerminalSession;

impl TerminalSession {
    pub(super) fn enter() -> io::Result<(TuiTerminal, Self)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            DisableAlternateScroll,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = TerminalSurface::new(backend)?;
        terminal.hide_cursor()?;
        Ok((terminal, Self))
    }

    pub(super) fn suspend(terminal: &mut TuiTerminal) -> io::Result<()> {
        terminal.show_cursor()?;
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            DisableBracketedPaste,
            DisableMouseCapture,
            DisableAlternateScroll,
            LeaveAlternateScreen
        )?;
        Ok(())
    }

    pub(super) fn resume(terminal: &mut TuiTerminal) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            DisableAlternateScroll,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            DisableAlternateScroll,
            LeaveAlternateScreen
        );
    }
}

#[cfg(test)]
mod tests {
    use crossterm::Command;

    use super::{DisableAlternateScroll, EnableAlternateScroll, TerminalMouseMode};

    #[test]
    fn alternate_scroll_commands_emit_xterm_mode_sequences() {
        let mut enable = String::new();
        EnableAlternateScroll.write_ansi(&mut enable).unwrap();
        assert_eq!(enable, "\x1b[?1007h");

        let mut disable = String::new();
        DisableAlternateScroll.write_ansi(&mut disable).unwrap();
        assert_eq!(disable, "\x1b[?1007l");
    }

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
