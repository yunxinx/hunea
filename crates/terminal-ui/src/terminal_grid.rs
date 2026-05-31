use ratatui::{
    buffer::{Buffer, Cell},
    layout::Position,
    style::Color,
};

use crate::display_width::display_width;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalDrawCommand<'a> {
    Put { x: u16, y: u16, cell: &'a Cell },
    ClearToEnd { x: u16, y: u16, bg: Color },
}

pub(crate) fn diff_terminal_buffers<'a>(
    previous: &Buffer,
    current: &'a Buffer,
) -> Vec<TerminalDrawCommand<'a>> {
    let previous_cells = &previous.content;
    let current_cells = &current.content;
    let mut commands = Vec::new();
    let mut last_content_columns = vec![None; current.area.height as usize];

    for y in 0..current.area.height {
        let row_start = y as usize * current.area.width as usize;
        let row_end = row_start + current.area.width as usize;
        let current_row = &current_cells[row_start..row_end];
        let previous_row = &previous_cells[row_start..row_end];
        let row_trailing_bg = current_row.last().map_or(Color::Reset, |cell| cell.bg);
        let clear_from = row_clear_from(current_row, row_trailing_bg);

        if clear_from > 0 {
            last_content_columns[y as usize] = Some(clear_from.saturating_sub(1) as u16);
        }

        if clear_from < current_row.len()
            && previous_tail_needs_clear(previous_row, clear_from, row_trailing_bg)
        {
            let (x, y) = current.pos_of(row_start + clear_from);
            commands.push(TerminalDrawCommand::ClearToEnd {
                x,
                y,
                bg: row_trailing_bg,
            });
        }
    }

    let mut invalidated = 0usize;
    let mut to_skip = 0usize;

    for (index, (current_cell, previous_cell)) in
        current_cells.iter().zip(previous_cells.iter()).enumerate()
    {
        if !current_cell.skip && (current_cell != previous_cell || invalidated > 0) && to_skip == 0
        {
            let (x, y) = current.pos_of(index);
            let row = index / current.area.width as usize;
            if last_content_columns[row].is_some_and(|last_content_column| x <= last_content_column)
            {
                commands.push(TerminalDrawCommand::Put {
                    x,
                    y,
                    cell: current_cell,
                });
            }
        }

        to_skip = display_width(current_cell.symbol()).saturating_sub(1);

        let affected_width =
            display_width(current_cell.symbol()).max(display_width(previous_cell.symbol()));
        invalidated = affected_width.max(invalidated).saturating_sub(1);
    }

    commands
}

fn row_clear_from(row: &[Cell], trailing_bg: Color) -> usize {
    let mut content_end = 0usize;
    let mut column = 0usize;

    while column < row.len() {
        let cell = &row[column];
        let width = display_width(cell.symbol()).max(1);
        if cell_requires_visible_cell(cell, trailing_bg) {
            content_end = column.saturating_add(width).min(row.len());
        }
        column += width;
    }

    content_end
}

fn previous_tail_needs_clear(row: &[Cell], clear_from: usize, clear_bg: Color) -> bool {
    let mut column = 0usize;
    while column < row.len() {
        let cell = &row[column];
        let width = display_width(cell.symbol()).max(1);
        if column < clear_from && column.saturating_add(width) > clear_from {
            return true;
        }
        column += width;
    }

    row[clear_from..]
        .iter()
        .any(|cell| cell_requires_clear(cell, clear_bg))
}

fn cell_requires_visible_cell(cell: &Cell, trailing_bg: Color) -> bool {
    cell.symbol() != " " || cell.bg != trailing_bg || !cell.modifier.is_empty()
}

fn cell_requires_clear(cell: &Cell, clear_bg: Color) -> bool {
    cell.symbol() != " " || cell.bg != clear_bg || !cell.modifier.is_empty()
}

/// 归一化 Ratatui `Buffer` 中宽 grapheme 的隐藏尾格。
pub(crate) fn normalize_terminal_buffer(buffer: &mut Buffer) {
    for cell in &mut buffer.content {
        cell.set_skip(false);
    }

    let area = buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let symbol = buffer[(x, y)].symbol();
            let width = display_width(symbol);
            if width <= 1 {
                continue;
            }

            let leading_style = buffer[(x, y)].style();
            for offset in 1..width {
                let Ok(offset) = u16::try_from(offset) else {
                    break;
                };
                let Some(trailing_x) = x.checked_add(offset) else {
                    break;
                };
                if trailing_x >= area.right() {
                    break;
                }

                let trailing = &mut buffer[Position::new(trailing_x, y)];
                if trailing.symbol() != " " {
                    break;
                }
                trailing.fg = leading_style.fg.unwrap_or(ratatui::style::Color::Reset);
                trailing.bg = leading_style.bg.unwrap_or(ratatui::style::Color::Reset);
                trailing.underline_color = leading_style
                    .underline_color
                    .unwrap_or(ratatui::style::Color::Reset);
                trailing.modifier = leading_style.add_modifier;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier, Style},
    };

    use super::normalize_terminal_buffer;

    #[test]
    fn normalize_terminal_buffer_preserves_keycap_trailing_cell_style() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer.set_string(
            0,
            0,
            "2️⃣",
            Style::default()
                .underline_color(Color::Red)
                .add_modifier(Modifier::REVERSED | Modifier::UNDERLINED),
        );

        normalize_terminal_buffer(&mut buffer);

        assert_eq!(buffer[(0, 0)].symbol(), "2️⃣");
        assert_eq!(buffer[(1, 0)].symbol(), " ");
        assert!(!buffer[(1, 0)].skip);
        assert!(buffer[(1, 0)].modifier.contains(Modifier::REVERSED));
        assert!(buffer[(1, 0)].modifier.contains(Modifier::UNDERLINED));
        assert_eq!(buffer[(1, 0)].underline_color, Color::Red);
    }

    #[test]
    fn terminal_grid_diff_skips_keycap_trailing_blank_without_skip() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        previous.set_string(0, 0, "ab", Style::default());
        let mut next = Buffer::empty(Rect::new(0, 0, 4, 1));
        next.set_string(0, 0, "2️⃣", Style::default());

        normalize_terminal_buffer(&mut next);
        next[(1, 0)].set_skip(false);
        let diff = super::diff_terminal_buffers(&previous, &next);
        let updates = diff
            .iter()
            .filter_map(|command| match command {
                super::TerminalDrawCommand::Put { x, y, cell } => Some((*x, *y, cell.symbol())),
                super::TerminalDrawCommand::ClearToEnd { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(updates, vec![(0, 0, "2️⃣")]);
    }

    #[test]
    fn terminal_grid_diff_put_commands_borrow_current_cells() {
        let previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        let mut current = Buffer::empty(Rect::new(0, 0, 4, 1));
        current.set_string(0, 0, "x", Style::default());

        let diff = super::diff_terminal_buffers(&previous, &current);

        let borrowed_cell = diff
            .iter()
            .find_map(|command| match command {
                super::TerminalDrawCommand::Put { cell, .. } => {
                    let cell: &ratatui::buffer::Cell = cell;
                    Some(cell)
                }
                super::TerminalDrawCommand::ClearToEnd { .. } => None,
            })
            .expect("changed cell should be emitted as Put");
        assert!(std::ptr::eq(borrowed_cell, &current[(0, 0)]));
    }

    #[test]
    fn normalize_terminal_buffer_keeps_nonblank_adjacent_cell_visible() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 4, 1));
        buffer[(0, 0)].set_symbol("2️⃣");
        buffer[(1, 0)].set_symbol("x");
        buffer[(1, 0)].set_skip(true);

        normalize_terminal_buffer(&mut buffer);

        assert_eq!(buffer[(1, 0)].symbol(), "x");
        assert!(!buffer[(1, 0)].skip);
    }

    #[test]
    fn terminal_grid_diff_does_not_clear_unchanged_empty_rows() {
        let previous = Buffer::empty(Rect::new(0, 0, 4, 2));
        let current = previous.clone();

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(
            diff.is_empty(),
            "unchanged empty buffers must not emit ANSI work: {diff:?}"
        );
    }

    #[test]
    fn terminal_grid_diff_clears_tail_only_when_previous_tail_needs_erasing() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        previous.set_string(0, 0, "abcd", Style::default());
        let mut current = Buffer::empty(Rect::new(0, 0, 4, 1));
        current.set_string(0, 0, "a", Style::default());

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::ClearToEnd {
                x: 1,
                y: 0,
                bg: Color::Reset,
            }
        )));
    }
}
