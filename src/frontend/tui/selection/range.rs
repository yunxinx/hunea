use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::SelectionState;

/// `SelectableLineRange` 描述一条渲染行里真正可落点、可复制的正文列范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectableLineRange {
    pub(crate) start_column: usize,
    pub(crate) end_column: usize,
    pub(crate) anchor_start_column: usize,
    pub(crate) anchor_end_column: usize,
}

impl SelectableLineRange {
    pub(crate) fn new(start_column: usize, end_column: usize) -> Self {
        if end_column <= start_column {
            return Self::default();
        }

        Self {
            start_column,
            end_column,
            anchor_start_column: start_column,
            anchor_end_column: end_column,
        }
    }

    pub(crate) fn blank_anchor(anchor_start_column: usize, anchor_end_column: usize) -> Self {
        if anchor_end_column <= anchor_start_column {
            return Self::default();
        }

        Self {
            anchor_start_column,
            anchor_end_column,
            ..Self::default()
        }
    }

    pub(crate) fn has_content(self) -> bool {
        self.end_column > self.start_column
    }

    pub(crate) fn has_anchor(self) -> bool {
        self.anchor_end_column > self.anchor_start_column
    }

    pub(crate) fn contains(self, column: usize) -> bool {
        self.has_anchor() && self.anchor_start_column <= column && column < self.anchor_end_column
    }

    pub(crate) fn clamp(self, column: usize) -> usize {
        if !self.has_content() {
            return 0;
        }
        if column < self.start_column {
            return self.start_column;
        }
        if column > self.end_column {
            return self.end_column;
        }

        column
    }
}

pub(crate) fn selectable_range_for_plain_line(text: &str) -> SelectableLineRange {
    SelectableLineRange::new(0, text.width())
}

pub(crate) fn normalize_transcript_selectable_range(
    text: &str,
    width: usize,
    preserves_blank_anchor: bool,
) -> SelectableLineRange {
    let range = selectable_range_for_plain_line(text);
    if range.has_content() {
        return range;
    }

    if preserves_blank_anchor && width > 0 {
        return SelectableLineRange::blank_anchor(0, width);
    }

    SelectableLineRange::default()
}

pub(crate) fn selection_columns_for_line(
    selection: SelectionState,
    line: usize,
    selectable: SelectableLineRange,
) -> Option<(usize, usize)> {
    let (start, end) = selection.ordered_points()?;
    if line < start.line || line > end.line || !selectable.has_content() {
        return None;
    }

    let (start_column, end_column) = if start.line == end.line {
        (selectable.clamp(start.column), selectable.clamp(end.column))
    } else if line == start.line {
        (selectable.clamp(start.column), selectable.end_column)
    } else if line == end.line {
        (selectable.start_column, selectable.clamp(end.column))
    } else {
        (selectable.start_column, selectable.end_column)
    };

    (start_column < end_column).then_some((start_column, end_column))
}

pub(crate) fn selection_ends_before_line_content(
    selection: SelectionState,
    line: usize,
    selectable: SelectableLineRange,
) -> bool {
    let Some((start, end)) = selection.ordered_points() else {
        return false;
    };
    if start.line >= end.line || line != end.line {
        return false;
    }

    if !selectable.has_content() {
        return end.column == 0;
    }

    end.column <= selectable.start_column
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VisibleTextCell {
    start_column: usize,
    end_column: usize,
    start_byte: usize,
    end_byte: usize,
}

pub(crate) fn word_selection_columns(line: &str, column: usize) -> Option<(usize, usize)> {
    let cells = visible_text_cells(line);
    if cells.is_empty() {
        return None;
    }

    let target_byte_offset = byte_offset_for_column(&cells, column)?;
    selection_columns_for_word_containing_byte_offset(line, &cells, target_byte_offset).or_else(
        || {
            (target_byte_offset == line.len())
                .then(|| cells.last().map(|cell| cell.start_byte))
                .flatten()
                .and_then(|last_byte| {
                    selection_columns_for_word_containing_byte_offset(line, &cells, last_byte)
                })
        },
    )
}

fn selection_columns_for_word_containing_byte_offset(
    line: &str,
    cells: &[VisibleTextCell],
    target_byte_offset: usize,
) -> Option<(usize, usize)> {
    for (start_byte, segment) in line.split_word_bound_indices() {
        let end_byte = start_byte + segment.len();
        if segment.chars().all(char::is_whitespace) {
            continue;
        }
        if start_byte <= target_byte_offset && target_byte_offset < end_byte {
            return selection_columns_for_word_byte_range(cells, start_byte, end_byte);
        }
    }

    None
}

fn selection_columns_for_word_byte_range(
    cells: &[VisibleTextCell],
    start_byte: usize,
    end_byte: usize,
) -> Option<(usize, usize)> {
    let start_column = column_for_byte_offset(cells, start_byte)?;
    let end_column = column_for_byte_offset(cells, end_byte)?;
    (start_column < end_column).then_some((start_column, end_column))
}

fn visible_text_cells(line: &str) -> Vec<VisibleTextCell> {
    let mut cells = Vec::new();
    let mut column = 0;
    for (start_byte, grapheme) in line.grapheme_indices(true) {
        let width = grapheme.width();
        let end_byte = start_byte + grapheme.len();
        if width == 0 {
            continue;
        }
        cells.push(VisibleTextCell {
            start_column: column,
            end_column: column + width,
            start_byte,
            end_byte,
        });
        column += width;
    }
    cells
}

fn byte_offset_for_column(cells: &[VisibleTextCell], column: usize) -> Option<usize> {
    if cells.is_empty() {
        return None;
    }
    if column == 0 {
        return Some(0);
    }

    let last = *cells.last()?;
    if column >= last.end_column {
        return Some(last.end_byte);
    }

    cells
        .iter()
        .find(|cell| cell.start_column <= column && column < cell.end_column)
        .map(|cell| cell.start_byte)
}

fn column_for_byte_offset(cells: &[VisibleTextCell], byte_offset: usize) -> Option<usize> {
    if cells.is_empty() {
        return (byte_offset == 0).then_some(0);
    }
    if byte_offset == 0 {
        return Some(0);
    }

    let last = *cells.last()?;
    if byte_offset == last.end_byte {
        return Some(last.end_column);
    }

    for cell in cells {
        if cell.start_byte == byte_offset {
            return Some(cell.start_column);
        }
        if cell.end_byte == byte_offset {
            return Some(cell.end_column);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::selection::{SelectionPoint, SelectionState};

    #[test]
    fn blank_anchor_can_accept_mouse_hit_without_copying_fill() {
        let range = SelectableLineRange::blank_anchor(0, 8);

        assert!(range.contains(0));
        assert!(range.contains(7));
        assert!(!range.has_content());
        assert_eq!(range.clamp(3), 0);
    }

    #[test]
    fn selection_columns_respect_single_line_range() {
        let selection = SelectionState {
            active: true,
            dragging: false,
            anchor: SelectionPoint { line: 1, column: 2 },
            focus: SelectionPoint { line: 1, column: 5 },
        };

        assert_eq!(
            selection_columns_for_line(selection, 1, SelectableLineRange::new(0, 10)),
            Some((2, 5))
        );
    }

    #[test]
    fn word_selection_columns_skip_zero_width_clusters() {
        assert_eq!(
            word_selection_columns("alpha\u{200b}beta gamma", 5),
            Some((5, 9))
        );
    }
}
