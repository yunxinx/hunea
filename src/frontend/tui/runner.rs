use std::{
    io,
    time::{Duration, Instant},
};

use color_eyre::eyre::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::{AppEvent, HeroOptions, Model, STARTUP_PROBE_TIMEOUT, theme};

/// `run` 启动交互式 TUI，并在退出后返回最终模型。
pub fn run(hero_options: HeroOptions) -> Result<Model> {
    let mut model = Model::new(hero_options);

    if let Some(detection) = theme::try_detect_palette() {
        model.update(AppEvent::DetectedPalette {
            palette: detection.palette,
            has_dark_background: detection.has_dark_background,
        });
    }

    let (mut terminal, _guard) = TerminalSession::enter()?;
    let area = terminal.size()?;
    model.update(AppEvent::Resized {
        width: area.width,
        height: area.height,
    });

    let startup_deadline = Instant::now() + STARTUP_PROBE_TIMEOUT;

    loop {
        terminal.draw(|frame| model.render(frame))?;

        if model.is_quitting() {
            break;
        }

        if !model.has_palette() && Instant::now() >= startup_deadline {
            model.update(AppEvent::StartupReadyTimeout);
            continue;
        }

        let wait_duration = if model.has_palette() {
            Duration::from_millis(250)
        } else {
            startup_deadline.saturating_duration_since(Instant::now())
        };

        if !event::poll(wait_duration)? {
            if !model.has_palette() {
                model.update(AppEvent::StartupReadyTimeout);
            }
            continue;
        }

        match event::read()? {
            Event::Key(key) => model.update(AppEvent::Key(key)),
            Event::Resize(width, height) => model.update(AppEvent::Resized { width, height }),
            _ => {}
        }
    }

    Ok(model)
}

struct TerminalSession;

impl TerminalSession {
    fn enter() -> io::Result<(Terminal<CrosstermBackend<io::Stdout>>, Self)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok((terminal, Self))
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}
