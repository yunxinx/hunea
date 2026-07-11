use ratatui::{buffer::Buffer, layout::Rect};

use super::*;
use crate::theme::default_palette;

mod animation;
mod model;
mod render;
mod state;

fn show_notice(state: &mut ToastState, notice: &ToastNotice) {
    state.show(notice.severity, notice.text.clone());
}

fn start_notice_exit(
    state: &mut ToastState,
    notice: &ToastNotice,
    started_at: Instant,
    bounds: Rect,
    palette: crate::theme::TerminalPalette,
) -> Instant {
    show_notice(state, notice);
    let visible_at = complete_current_enter(state, started_at, bounds, palette);
    let token = state.timeout_token();
    state.handle_visible_timeout(token, true);
    state.advance_at(visible_at);
    visible_at
}

fn complete_current_enter(
    state: &mut ToastState,
    started_at: Instant,
    bounds: Rect,
    palette: crate::theme::TerminalPalette,
) -> Instant {
    let mut buffer = Buffer::empty(bounds);
    state.render_at(started_at, bounds, &mut buffer, palette);
    state.advance_at(started_at);
    let visible_at = started_at + TOAST_ENTER_DURATION;
    state.render_at(visible_at, bounds, &mut buffer, palette);
    state.advance_at(visible_at);
    visible_at
}

fn complete_current_exit(
    state: &mut ToastState,
    started_at: Instant,
    bounds: Rect,
    palette: crate::theme::TerminalPalette,
) -> Instant {
    let mut buffer = Buffer::empty(bounds);
    state.render_at(started_at, bounds, &mut buffer, palette);
    state.advance_at(started_at);
    let completed_at = started_at + TOAST_EXIT_DURATION;
    state.render_at(completed_at, bounds, &mut buffer, palette);
    state.advance_at(completed_at);
    completed_at
}

fn fill_underlay(buffer: &mut Buffer, symbol: &str) {
    for y in buffer.area.y..buffer.area.bottom() {
        for x in buffer.area.x..buffer.area.right() {
            buffer[(x, y)].set_symbol(symbol);
        }
    }
}

fn assert_rect_symbols(buffer: &Buffer, area: Rect, symbol: &str) {
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            assert_eq!(
                buffer[(x, y)].symbol(),
                symbol,
                "cell at ({x}, {y}) should restore the underlying frame"
            );
        }
    }
}

fn assert_reset_blank_cell(buffer: &Buffer, x: u16, y: u16) {
    let cell = &buffer[(x, y)];
    assert_eq!(cell.symbol(), " ");
    assert_eq!(cell.fg, Color::Reset);
    assert_eq!(cell.bg, Color::Reset);
}

fn count_reset_blank_columns_on_row(buffer: &Buffer, area: Rect, y: u16) -> usize {
    (area.x..area.right())
        .filter(|x| {
            let cell = &buffer[(*x, y)];
            cell.symbol() == " " && cell.fg == Color::Reset && cell.bg == Color::Reset
        })
        .count()
}

fn find_symbol_on_row(buffer: &Buffer, area: Rect, y: u16, symbol: &str) -> Option<u16> {
    (area.x..area.right()).find(|x| buffer[(*x, y)].symbol() == symbol)
}
