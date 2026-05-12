use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::document::{DocumentLayout, DocumentLineAnchor};

use super::{SelectionPoint, SelectionState};

/// `SelectableLineRange` 描述一条渲染行的可选择列范围。
///
/// `content_*` 是真正参与复制与高亮的正文范围；`hit_*` 是允许鼠标按下或拖拽
/// 锚定选择的范围。二者分离后，左侧提示符、视觉缩进、状态行内边距等区域可以
/// 作为更容易命中的选择手柄，但不会混入复制文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SelectableLineRange {
    content_start_column: usize,
    content_end_column: usize,
    hit_start_column: usize,
    hit_end_column: usize,
}

impl SelectableLineRange {
    pub(crate) fn new(start_column: usize, end_column: usize) -> Self {
        if end_column <= start_column {
            return Self::default();
        }

        Self {
            content_start_column: start_column,
            content_end_column: end_column,
            hit_start_column: start_column,
            hit_end_column: end_column,
        }
    }

    pub(crate) fn with_hit_range(
        start_column: usize,
        end_column: usize,
        hit_start_column: usize,
        hit_end_column: usize,
    ) -> Self {
        if end_column <= start_column || hit_end_column <= hit_start_column {
            return Self::default();
        }

        Self {
            content_start_column: start_column,
            content_end_column: end_column,
            hit_start_column,
            hit_end_column,
        }
    }

    pub(crate) fn blank_hit_range(hit_start_column: usize, hit_end_column: usize) -> Self {
        if hit_end_column <= hit_start_column {
            return Self::default();
        }

        Self {
            hit_start_column,
            hit_end_column,
            ..Self::default()
        }
    }

    pub(crate) fn has_content(self) -> bool {
        self.content_end_column > self.content_start_column
    }

    pub(crate) fn has_hit_range(self) -> bool {
        self.hit_end_column > self.hit_start_column
    }

    pub(crate) fn content_columns(self) -> Option<(usize, usize)> {
        self.has_content()
            .then_some((self.content_start_column, self.content_end_column))
    }

    #[cfg(test)]
    pub(crate) fn hit_columns(self) -> Option<(usize, usize)> {
        self.has_hit_range()
            .then_some((self.hit_start_column, self.hit_end_column))
    }

    pub(crate) fn contains_hit(self, column: usize) -> bool {
        self.has_hit_range() && self.hit_start_column <= column && column < self.hit_end_column
    }

    pub(crate) fn contains_content(self, column: usize) -> bool {
        self.has_content()
            && self.content_start_column <= column
            && column < self.content_end_column
    }

    pub(crate) fn clamp_to_content(self, column: usize) -> usize {
        if !self.has_content() {
            return 0;
        }
        if column < self.content_start_column {
            return self.content_start_column;
        }
        if column > self.content_end_column {
            return self.content_end_column;
        }

        column
    }

    pub(crate) fn point_for_mouse_down(
        self,
        anchor: DocumentLineAnchor,
        column: usize,
    ) -> Option<SelectionPoint> {
        if !self.contains_hit(column) {
            return None;
        }

        Some(SelectionPoint::new(
            anchor,
            if self.has_content() { column } else { 0 },
        ))
    }

    pub(crate) fn point_for_drag(
        self,
        anchor: DocumentLineAnchor,
        column: usize,
    ) -> Option<SelectionPoint> {
        self.has_hit_range()
            .then_some(SelectionPoint::new(anchor, self.clamp_to_content(column)))
    }
}

pub(crate) fn selectable_range_for_plain_line(text: &str) -> SelectableLineRange {
    SelectableLineRange::new(0, text.width())
}

pub(crate) fn normalize_transcript_selectable_range(
    text: &str,
    width: usize,
    preserves_blank_hit_range: bool,
) -> SelectableLineRange {
    let range = selectable_range_for_plain_line(text);
    if range.has_content() {
        return range;
    }

    if preserves_blank_hit_range && width > 0 {
        return SelectableLineRange::blank_hit_range(0, width);
    }

    SelectableLineRange::default()
}

pub(crate) fn selection_columns_for_line(
    selection: SelectionState,
    layout: &DocumentLayout,
    line: usize,
    selectable: SelectableLineRange,
) -> Option<(usize, usize)> {
    let (start, end) = selection.ordered_points(layout)?;
    if line < start.line() || line > end.line() || !selectable.has_content() {
        return None;
    }

    let (start_column, end_column) = if start.line() == end.line() {
        (
            selectable.clamp_to_content(start.column()),
            selectable.clamp_to_content(end.column()),
        )
    } else if line == start.line() {
        let (_, end_column) = selectable
            .content_columns()
            .expect("content columns checked above");
        (selectable.clamp_to_content(start.column()), end_column)
    } else if line == end.line() {
        let (start_column, _) = selectable
            .content_columns()
            .expect("content columns checked above");
        (start_column, selectable.clamp_to_content(end.column()))
    } else {
        selectable
            .content_columns()
            .expect("content columns checked above")
    };

    (start_column < end_column).then_some((start_column, end_column))
}

pub(crate) fn selection_ends_before_line_content(
    selection: SelectionState,
    layout: &DocumentLayout,
    line: usize,
    selectable: SelectableLineRange,
) -> bool {
    let Some((start, end)) = selection.ordered_points(layout) else {
        return false;
    };
    if start.line() >= end.line() || line != end.line() {
        return false;
    }

    if !selectable.has_content() {
        return end.column() == 0;
    }

    let (start_column, _) = selectable
        .content_columns()
        .expect("content columns checked above");
    end.column() <= start_column
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
    use crate::frontend::tui::document::DocumentLayout;
    use crate::frontend::tui::selection::{SelectionPoint, SelectionState};

    fn selection_test_layout(line_count: usize) -> DocumentLayout {
        let lines = vec![""; line_count];
        DocumentLayout::with_test_plain_lines(0, &lines)
    }

    #[test]
    fn selectable_range_reports_semantic_content_and_hit_bounds() {
        let content = SelectableLineRange::new(2, 6);
        assert_eq!(content.content_columns(), Some((2, 6)));
        assert_eq!(content.hit_columns(), Some((2, 6)));

        let blank = SelectableLineRange::blank_hit_range(0, 8);
        assert_eq!(blank.content_columns(), None);
        assert_eq!(blank.hit_columns(), Some((0, 8)));
    }

    #[test]
    fn blank_hit_range_can_accept_mouse_hit_without_copying_fill() {
        let range = SelectableLineRange::blank_hit_range(0, 8);

        assert!(range.contains_hit(0));
        assert!(range.contains_hit(7));
        assert!(!range.has_content());
        assert_eq!(range.clamp_to_content(3), 0);
        assert_eq!(
            range
                .point_for_mouse_down(DocumentLineAnchor::default(), 3)
                .expect("blank hit range should accept mouse down")
                .column(),
            0
        );
        assert_eq!(
            range
                .point_for_drag(DocumentLineAnchor::default(), 3)
                .expect("blank hit range should accept drag")
                .column(),
            0
        );
    }

    #[test]
    fn hit_range_can_extend_before_content_without_copying_prefix() {
        let range = SelectableLineRange::with_hit_range(2, 6, 0, 6);
        assert_eq!(range.content_columns(), Some((2, 6)));
        assert_eq!(range.hit_columns(), Some((0, 6)));
        assert!(range.contains_hit(0));
        assert!(!range.contains_content(0));
        let layout = selection_test_layout(1);
        let anchor = layout.line_anchor_at(0).expect("line anchor");

        let hit = range
            .point_for_mouse_down(anchor, 0)
            .expect("prompt area should be a valid hit target");
        assert_eq!(hit.column(), 0);

        let mut selection = SelectionState::default();
        selection.select_range(hit, SelectionPoint::new(hit.anchor(), 6));
        assert_eq!(
            selection_columns_for_line(selection, &layout, 0, range),
            Some((2, 6))
        );
    }

    #[test]
    fn drag_point_clamps_to_content_range() {
        let range = SelectableLineRange::with_hit_range(2, 6, 0, 8);
        let anchor = DocumentLineAnchor::default();

        assert_eq!(
            range
                .point_for_drag(anchor, 0)
                .expect("drag should clamp before content")
                .column(),
            2
        );
        assert_eq!(
            range
                .point_for_drag(anchor, 8)
                .expect("drag should clamp after content")
                .column(),
            6
        );
    }

    #[test]
    fn selection_columns_respect_single_line_range() {
        let layout = selection_test_layout(2);
        let mut selection = SelectionState::default();
        selection.select_range(
            SelectionPoint::new(layout.line_anchor_at(1).expect("line anchor"), 2),
            SelectionPoint::new(layout.line_anchor_at(1).expect("line anchor"), 5),
        );

        assert_eq!(
            selection_columns_for_line(selection, &layout, 1, SelectableLineRange::new(0, 10)),
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
