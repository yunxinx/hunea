use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{self, Event, MouseEventKind};

use crate::{AppEvent, Model};

const MAX_READY_TERMINAL_EVENTS_PER_FRAME: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalInputAction {
    App(AppEvent),
    CancelExitConfirmation,
}

pub(super) fn read_ready_terminal_events(first_event: Event) -> Result<Vec<Event>> {
    let mut events = vec![first_event];
    while events.len() < MAX_READY_TERMINAL_EVENTS_PER_FRAME && event::poll(Duration::ZERO)? {
        events.push(event::read()?);
    }
    Ok(events)
}

pub(super) fn coalesced_input_actions(
    events: impl IntoIterator<Item = Event>,
) -> Vec<TerminalInputAction> {
    let mut actions = Vec::new();
    let mut pending_wheel_delta = 0_isize;

    for event in events {
        match event {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    pending_wheel_delta -= Model::document_mouse_wheel_delta();
                }
                MouseEventKind::ScrollDown => {
                    pending_wheel_delta += Model::document_mouse_wheel_delta();
                }
                MouseEventKind::Down(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseDown {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                MouseEventKind::Up(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseUp {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                MouseEventKind::Drag(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseDrag {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                _ => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::CancelExitConfirmation);
                }
            },
            Event::Key(key) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Key(key)));
            }
            Event::Paste(text) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Paste(text)));
            }
            Event::Resize(width, height) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Resized {
                    width,
                    height,
                }));
            }
            _ => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
            }
        }
    }

    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
    actions
}

fn flush_pending_wheel_delta(actions: &mut Vec<TerminalInputAction>, delta: &mut isize) {
    if *delta == 0 {
        return;
    }

    actions.push(TerminalInputAction::App(AppEvent::MouseWheel {
        delta_lines: *delta,
    }));
    *delta = 0;
}
