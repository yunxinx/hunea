use crossterm::event::{Event, KeyCode, KeyEvent, MouseEventKind};

use crate::{AppEvent, Model};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalInputAction {
    App(AppEvent),
    CancelExitConfirmation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TerminalInputCoalescing {
    pub(crate) has_page_scroll_burst_coalescing: bool,
}

#[cfg(test)]
pub(super) fn coalesced_input_actions(
    events: impl IntoIterator<Item = Event>,
) -> Vec<TerminalInputAction> {
    coalesced_input_actions_with_options(events, TerminalInputCoalescing::default())
}

pub(super) fn coalesced_input_actions_with_options(
    events: impl IntoIterator<Item = Event>,
    options: TerminalInputCoalescing,
) -> Vec<TerminalInputAction> {
    let mut actions = Vec::new();
    let mut pending_wheel_delta = 0_isize;
    let mut pending_alternate_scroll_delta = 0_isize;

    for event in events {
        push_regular_event(
            event,
            &mut actions,
            &mut pending_wheel_delta,
            &mut pending_alternate_scroll_delta,
            options,
        );
    }

    flush_pending_scroll_deltas(
        &mut actions,
        &mut pending_wheel_delta,
        &mut pending_alternate_scroll_delta,
        options,
    );
    actions
}

fn push_regular_event(
    event: Event,
    actions: &mut Vec<TerminalInputAction>,
    pending_wheel_delta: &mut isize,
    pending_alternate_scroll_delta: &mut isize,
    options: TerminalInputCoalescing,
) {
    match event {
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollUp => {
                flush_pending_alternate_scroll_delta(actions, pending_alternate_scroll_delta);
                *pending_wheel_delta -= Model::document_mouse_wheel_delta();
            }
            MouseEventKind::ScrollDown => {
                flush_pending_alternate_scroll_delta(actions, pending_alternate_scroll_delta);
                *pending_wheel_delta += Model::document_mouse_wheel_delta();
            }
            MouseEventKind::Down(button) => {
                flush_pending_scroll_deltas(
                    actions,
                    pending_wheel_delta,
                    pending_alternate_scroll_delta,
                    options,
                );
                actions.push(TerminalInputAction::App(AppEvent::MouseDown {
                    button,
                    column: mouse.column,
                    row: mouse.row,
                }));
            }
            MouseEventKind::Up(button) => {
                flush_pending_scroll_deltas(
                    actions,
                    pending_wheel_delta,
                    pending_alternate_scroll_delta,
                    options,
                );
                actions.push(TerminalInputAction::App(AppEvent::MouseUp {
                    button,
                    column: mouse.column,
                    row: mouse.row,
                }));
            }
            MouseEventKind::Drag(button) => {
                flush_pending_scroll_deltas(
                    actions,
                    pending_wheel_delta,
                    pending_alternate_scroll_delta,
                    options,
                );
                actions.push(TerminalInputAction::App(AppEvent::MouseDrag {
                    button,
                    column: mouse.column,
                    row: mouse.row,
                }));
            }
            _ => {
                flush_pending_scroll_deltas(
                    actions,
                    pending_wheel_delta,
                    pending_alternate_scroll_delta,
                    options,
                );
                actions.push(TerminalInputAction::CancelExitConfirmation);
            }
        },
        Event::Key(key) => {
            if options.has_page_scroll_burst_coalescing
                && key.modifiers.is_empty()
                && matches!(key.code, KeyCode::Up | KeyCode::Down)
            {
                flush_pending_wheel_delta(actions, pending_wheel_delta, options);
                if key.code == KeyCode::Up {
                    *pending_alternate_scroll_delta -= 1;
                } else {
                    *pending_alternate_scroll_delta += 1;
                }
                return;
            }
            flush_pending_scroll_deltas(
                actions,
                pending_wheel_delta,
                pending_alternate_scroll_delta,
                options,
            );
            actions.push(TerminalInputAction::App(AppEvent::Key(key)));
        }
        Event::Paste(text) => {
            flush_pending_scroll_deltas(
                actions,
                pending_wheel_delta,
                pending_alternate_scroll_delta,
                options,
            );
            actions.push(TerminalInputAction::App(AppEvent::Paste(text)));
        }
        Event::Resize(width, height) => {
            flush_pending_scroll_deltas(
                actions,
                pending_wheel_delta,
                pending_alternate_scroll_delta,
                options,
            );
            actions.push(TerminalInputAction::App(AppEvent::Resized {
                width,
                height,
            }));
        }
        _ => {
            flush_pending_scroll_deltas(
                actions,
                pending_wheel_delta,
                pending_alternate_scroll_delta,
                options,
            );
        }
    }
}

fn flush_pending_scroll_deltas(
    actions: &mut Vec<TerminalInputAction>,
    wheel_delta: &mut isize,
    alternate_scroll_delta: &mut isize,
    options: TerminalInputCoalescing,
) {
    flush_pending_wheel_delta(actions, wheel_delta, options);
    flush_pending_alternate_scroll_delta(actions, alternate_scroll_delta);
}

fn flush_pending_wheel_delta(
    actions: &mut Vec<TerminalInputAction>,
    delta: &mut isize,
    options: TerminalInputCoalescing,
) {
    if *delta == 0 {
        return;
    }

    let delta_lines = if options.has_page_scroll_burst_coalescing {
        delta.signum()
    } else {
        *delta
    };
    actions.push(TerminalInputAction::App(AppEvent::MouseWheel {
        delta_lines,
    }));
    *delta = 0;
}

fn flush_pending_alternate_scroll_delta(actions: &mut Vec<TerminalInputAction>, delta: &mut isize) {
    if *delta == 0 {
        return;
    }

    let key_code = if delta.is_negative() {
        KeyCode::Up
    } else {
        KeyCode::Down
    };
    actions.push(TerminalInputAction::App(AppEvent::Key(KeyEvent::from(
        key_code,
    ))));
    *delta = 0;
}
