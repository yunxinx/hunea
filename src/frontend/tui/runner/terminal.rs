use std::io;

use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::event_pipeline::TerminalWaitPlan;

pub(super) type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

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

pub(super) struct TerminalSession;

impl TerminalSession {
    pub(super) fn enter() -> io::Result<(TuiTerminal, Self)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok((terminal, Self))
    }

    pub(super) fn suspend(terminal: &mut TuiTerminal) -> io::Result<()> {
        terminal.show_cursor()?;
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        Ok(())
    }

    pub(super) fn resume(terminal: &mut TuiTerminal) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
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
            LeaveAlternateScreen
        );
    }
}
