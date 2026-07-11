use ratatui::{
    buffer::{Buffer, Cell, CellDiffOption, CellWidth},
    style::{Color, Modifier},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalDrawCommand<'a> {
    Put {
        x: u16,
        y: u16,
        cell: &'a Cell,
        prefill_width: usize,
    },
    ClearToEnd {
        x: u16,
        y: u16,
        bg: Color,
    },
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
        let current_width = usize::from(current_cell.cell_width());
        let previous_width = usize::from(previous_cell.cell_width());
        if !cell_is_skipped(current_cell)
            && (cell_is_always_updated(current_cell)
                || current_cell != previous_cell
                || invalidated > 0)
            && to_skip == 0
        {
            let (x, y) = current.pos_of(index);
            let row = index / current.area.width as usize;
            if last_content_columns[row].is_some_and(|last_content_column| x <= last_content_column)
            {
                commands.push(TerminalDrawCommand::Put {
                    x,
                    y,
                    cell: current_cell,
                    prefill_width: wide_prefill_width(
                        previous_cells,
                        current_cells,
                        index,
                        current.area.width as usize,
                        current_width,
                    ),
                });
            }
        }

        to_skip = current_width.saturating_sub(1);

        let affected_width = current_width.max(previous_width);
        invalidated = affected_width.max(invalidated).saturating_sub(1);
    }

    commands
}

fn row_clear_from(row: &[Cell], trailing_bg: Color) -> usize {
    let mut content_end = 0usize;
    let mut column = 0usize;

    while column < row.len() {
        let cell = &row[column];
        let width = usize::from(cell.cell_width()).max(1);
        if !cell_is_skipped(cell) && cell_requires_visible_cell(cell, trailing_bg) {
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
        let width = usize::from(cell.cell_width()).max(1);
        if !cell_is_skipped(cell)
            && column < clear_from
            && column.saturating_add(width) > clear_from
        {
            return true;
        }
        column += width;
    }

    row[clear_from..]
        .iter()
        .any(|cell| !cell_is_skipped(cell) && cell_requires_clear(cell, clear_bg))
}

fn cell_is_skipped(cell: &Cell) -> bool {
    matches!(cell.diff_option, CellDiffOption::Skip)
}

fn cell_is_always_updated(cell: &Cell) -> bool {
    matches!(cell.diff_option, CellDiffOption::AlwaysUpdate)
}

fn cell_requires_visible_cell(cell: &Cell, trailing_bg: Color) -> bool {
    cell.symbol() != " " || cell.bg != trailing_bg || !cell.modifier.is_empty()
}

fn cell_requires_clear(cell: &Cell, clear_bg: Color) -> bool {
    cell.symbol() != " " || cell.bg != clear_bg || !cell.modifier.is_empty()
}

fn wide_prefill_width(
    previous_cells: &[Cell],
    current_cells: &[Cell],
    index: usize,
    row_width: usize,
    width: usize,
) -> usize {
    if width <= 1 || row_width == 0 {
        return 0;
    }

    let row_end = (index / row_width + 1) * row_width;
    let end = index
        .saturating_add(width)
        .min(row_end)
        .min(current_cells.len())
        .min(previous_cells.len());
    let leading = &current_cells[index];
    if leading.modifier.contains(Modifier::REVERSED)
        || current_cells[index + 1..end].iter().any(|cell| {
            cell.symbol() == " "
                && tail_has_visible_style(cell)
                && tail_style_matches_leading(cell, leading)
        })
        || previous_cells[index + 1..end]
            .iter()
            .zip(&current_cells[index + 1..end])
            .any(|(previous, current)| current.symbol() == " " && previous != current)
    {
        width
    } else {
        0
    }
}

fn tail_style_matches_leading(tail: &Cell, leading: &Cell) -> bool {
    tail.fg == leading.fg
        && tail.bg == leading.bg
        && tail.underline_color == leading.underline_color
        && tail.modifier == leading.modifier
}

fn tail_has_visible_style(cell: &Cell) -> bool {
    cell.fg != Color::Reset
        || cell.bg != Color::Reset
        || cell.underline_color != Color::Reset
        || !cell.modifier.is_empty()
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU16;

    use ratatui::{
        buffer::{Buffer, CellDiffOption},
        layout::Rect,
        style::{Color, Modifier, Style},
    };

    #[test]
    fn terminal_grid_diff_skips_keycap_trailing_blank_from_ratatui_buffer() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        previous.set_string(0, 0, "ab", Style::default());
        let mut next = Buffer::empty(Rect::new(0, 0, 4, 1));
        next.set_string(0, 0, "2️⃣", Style::default());

        let diff = super::diff_terminal_buffers(&previous, &next);
        let updates = diff
            .iter()
            .filter_map(|command| match command {
                super::TerminalDrawCommand::Put { x, y, cell, .. } => Some((*x, *y, cell.symbol())),
                super::TerminalDrawCommand::ClearToEnd { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(updates, vec![(0, 0, "2️⃣")]);
    }

    #[test]
    fn terminal_grid_diff_does_not_let_styled_hidden_wide_tail_extend_content() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 6, 1));
        previous.set_string(0, 0, "abcdef", Style::default());
        let mut current = Buffer::empty(Rect::new(0, 0, 6, 1));
        current.set_string(0, 0, "2️⃣", Style::default());
        current[(1, 0)]
            .set_style(Style::default().add_modifier(Modifier::REVERSED | Modifier::UNDERLINED));

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::ClearToEnd {
                x: 2,
                y: 0,
                bg: Color::Reset,
            }
        )));
        assert!(
            !diff.iter().any(|command| matches!(
                command,
                super::TerminalDrawCommand::Put { x: 1, y: 0, .. }
            ))
        );
    }

    #[test]
    fn terminal_grid_diff_respects_explicit_diff_option_skip_cells() {
        let previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        let mut current = Buffer::empty(Rect::new(0, 0, 4, 1));
        current.set_string(0, 0, "abcd", Style::default());
        current[(1, 0)].set_diff_option(CellDiffOption::Skip);

        let diff = super::diff_terminal_buffers(&previous, &current);
        let updates = diff
            .iter()
            .filter_map(|command| match command {
                super::TerminalDrawCommand::Put { x, y, cell, .. } => Some((*x, *y, cell.symbol())),
                super::TerminalDrawCommand::ClearToEnd { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(updates, vec![(0, 0, "a"), (2, 0, "c"), (3, 0, "d")]);
    }

    #[test]
    fn terminal_grid_diff_always_updates_equal_cells() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 2, 1));
        previous[(0, 0)].set_symbol("x");
        let mut current = previous.clone();
        current[(0, 0)].set_diff_option(CellDiffOption::AlwaysUpdate);
        previous[(0, 0)].set_diff_option(CellDiffOption::AlwaysUpdate);

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::Put { x: 0, y: 0, cell, .. }
                if cell.symbol() == "x"
        )));
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

    #[test]
    fn terminal_grid_diff_marks_styled_wide_tail_for_prefill() {
        let previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        let mut current = Buffer::empty(Rect::new(0, 0, 4, 1));
        current.set_string(
            0,
            0,
            "2️⃣",
            Style::default().add_modifier(Modifier::REVERSED),
        );
        current[(1, 0)].set_style(Style::default().add_modifier(Modifier::REVERSED));

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::Put {
                x: 0,
                y: 0,
                cell,
                prefill_width: 2,
            } if cell.symbol() == "2️⃣"
        )));
    }

    #[test]
    fn terminal_grid_diff_marks_changed_wide_tail_for_prefill() {
        let mut previous = Buffer::empty(Rect::new(0, 0, 4, 1));
        previous.set_string(0, 0, "ab", Style::default());
        let mut current = Buffer::empty(Rect::new(0, 0, 4, 1));
        current.set_string(0, 0, "2️⃣", Style::default());

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::Put {
                x: 0,
                y: 0,
                cell,
                prefill_width: 2,
            } if cell.symbol() == "2️⃣"
        )));
    }

    #[test]
    fn terminal_grid_diff_uses_forced_width_for_reversed_prefill() {
        let previous = Buffer::empty(Rect::new(0, 0, 5, 1));
        let mut current = Buffer::empty(Rect::new(0, 0, 5, 1));
        current[(0, 0)]
            .set_symbol("x")
            .set_style(Style::default().add_modifier(Modifier::REVERSED))
            .set_diff_option(CellDiffOption::ForcedWidth(
                NonZeroU16::new(3).expect("forced width must be non-zero"),
            ));

        let diff = super::diff_terminal_buffers(&previous, &current);

        assert!(diff.iter().any(|command| matches!(
            command,
            super::TerminalDrawCommand::Put {
                x: 0,
                y: 0,
                cell,
                prefill_width: 3,
            } if cell.symbol() == "x"
        )));
    }
}
