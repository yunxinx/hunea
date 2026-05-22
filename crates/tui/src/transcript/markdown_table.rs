use pulldown_cmark::Alignment;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const CELL_HORIZONTAL_PADDING: usize = 1;
const MIN_COLUMN_WIDTH: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TableCellKind {
    Border,
    Header,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TableLineSegment {
    pub text: String,
    pub kind: TableCellKind,
}

pub(super) type TableLine = Vec<TableLineSegment>;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct MarkdownTable {
    pub alignments: Vec<Alignment>,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub(super) fn render_markdown_table(
    table: &MarkdownTable,
    available_width: usize,
) -> Vec<TableLine> {
    let column_count = table_column_count(table);
    if column_count == 0 {
        return Vec::new();
    }

    let widths = table_column_widths(table, column_count, available_width.max(1));
    let mut lines = Vec::new();

    lines.push(border_line("┌", "┬", "┐", &widths));
    if !table.header.is_empty() {
        lines.extend(row_lines(
            &normalize_row(&table.header, column_count),
            &widths,
            &table.alignments,
            TableCellKind::Header,
        ));
        lines.push(border_line("├", "┼", "┤", &widths));
    }

    for row in &table.rows {
        lines.extend(row_lines(
            &normalize_row(row, column_count),
            &widths,
            &table.alignments,
            TableCellKind::Body,
        ));
    }

    lines.push(border_line("└", "┴", "┘", &widths));
    lines
}

fn table_column_count(table: &MarkdownTable) -> usize {
    table
        .alignments
        .len()
        .max(table.header.len())
        .max(table.rows.iter().map(Vec::len).max().unwrap_or(0))
}

fn table_column_widths(
    table: &MarkdownTable,
    column_count: usize,
    available_width: usize,
) -> Vec<usize> {
    let natural = natural_column_widths(table, column_count);
    let minimum = vec![MIN_COLUMN_WIDTH; column_count];
    let natural_total = table_total_width(&natural);
    if natural_total <= available_width {
        return natural;
    }

    let minimum_total = table_total_width(&minimum);
    if minimum_total >= available_width {
        return minimum;
    }

    let mut widths = minimum;
    let mut remaining = available_width - minimum_total;
    let mut expandable = (0..column_count)
        .filter(|index| natural[*index] > widths[*index])
        .collect::<Vec<_>>();

    while remaining > 0 && !expandable.is_empty() {
        let share = remaining.div_ceil(expandable.len()).max(1);
        let mut next_expandable = Vec::new();
        for index in expandable {
            if remaining == 0 {
                next_expandable.push(index);
                continue;
            }

            let room = natural[index] - widths[index];
            let added = room.min(share).min(remaining);
            widths[index] += added;
            remaining -= added;

            if widths[index] < natural[index] {
                next_expandable.push(index);
            }
        }
        expandable = next_expandable;
    }

    widths
}

fn natural_column_widths(table: &MarkdownTable, column_count: usize) -> Vec<usize> {
    let mut widths = vec![MIN_COLUMN_WIDTH; column_count];
    for (index, cell) in table.header.iter().enumerate().take(column_count) {
        widths[index] = widths[index].max(display_width(cell));
    }
    for row in &table.rows {
        for (index, cell) in row.iter().enumerate().take(column_count) {
            widths[index] = widths[index].max(display_width(cell));
        }
    }
    widths
}

fn table_total_width(column_widths: &[usize]) -> usize {
    if column_widths.is_empty() {
        return 0;
    }

    let content_width = column_widths
        .iter()
        .map(|width| width + CELL_HORIZONTAL_PADDING * 2)
        .sum::<usize>();
    content_width + column_widths.len() + 1
}

fn border_line(left: &str, middle: &str, right: &str, widths: &[usize]) -> TableLine {
    let mut segments = Vec::new();
    push_segment(&mut segments, left, TableCellKind::Border);
    for (index, width) in widths.iter().enumerate() {
        push_segment(
            &mut segments,
            "─".repeat(width + CELL_HORIZONTAL_PADDING * 2),
            TableCellKind::Border,
        );
        push_segment(
            &mut segments,
            if index + 1 == widths.len() {
                right
            } else {
                middle
            },
            TableCellKind::Border,
        );
    }
    segments
}

fn row_lines(
    row: &[String],
    widths: &[usize],
    alignments: &[Alignment],
    kind: TableCellKind,
) -> Vec<TableLine> {
    let wrapped_cells = row
        .iter()
        .zip(widths.iter())
        .map(|(cell, width)| wrap_cell(cell, *width))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let mut rendered = Vec::with_capacity(row_height);

    for visual_line_index in 0..row_height {
        let mut segments = Vec::new();
        push_segment(&mut segments, "│", TableCellKind::Border);
        for (cell_index, width) in widths.iter().copied().enumerate() {
            let text = wrapped_cells
                .get(cell_index)
                .and_then(|cell| cell.get(visual_line_index))
                .map(String::as_str)
                .unwrap_or("");
            push_segment(
                &mut segments,
                padded_cell(text, width, alignment_for_column(alignments, cell_index)),
                kind,
            );
            push_segment(&mut segments, "│", TableCellKind::Border);
        }
        rendered.push(segments);
    }

    rendered
}

fn normalize_row(row: &[String], column_count: usize) -> Vec<String> {
    (0..column_count)
        .map(|index| row.get(index).cloned().unwrap_or_default())
        .collect()
}

fn alignment_for_column(alignments: &[Alignment], index: usize) -> Alignment {
    alignments.get(index).copied().unwrap_or(Alignment::None)
}

fn padded_cell(text: &str, width: usize, alignment: Alignment) -> String {
    let text_width = display_width(text);
    let extra = width.saturating_sub(text_width);
    let (left, right) = match alignment {
        Alignment::Right => (extra, 0),
        Alignment::Center => (extra / 2, extra - extra / 2),
        Alignment::Left | Alignment::None => (0, extra),
    };

    format!(
        "{}{}{}{}",
        " ".repeat(CELL_HORIZONTAL_PADDING + left),
        text,
        " ".repeat(right),
        " ".repeat(CELL_HORIZONTAL_PADDING)
    )
}

fn wrap_cell(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    for source_line in text.split('\n') {
        let mut words = source_line.split_inclusive(char::is_whitespace).peekable();
        let mut current = String::new();
        let mut current_width = 0;

        while let Some(word) = words.next() {
            let word = word.trim_end_matches(char::is_whitespace);
            if word.is_empty() {
                continue;
            }

            let word_width = display_width(word);
            let separator_width = usize::from(!current.is_empty());
            if !current.is_empty() && current_width + separator_width + word_width <= width {
                current.push(' ');
                current.push_str(word);
                current_width += separator_width + word_width;
                continue;
            }

            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }

            if word_width <= width {
                current.push_str(word);
                current_width = word_width;
                continue;
            }

            let hard_wrapped = hard_wrap_word(word, width);
            let mut hard_iter = hard_wrapped.into_iter().peekable();
            while let Some(part) = hard_iter.next() {
                if hard_iter.peek().is_some() || words.peek().is_some() {
                    lines.push(part);
                } else {
                    current_width = display_width(&part);
                    current = part;
                }
            }
        }

        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn hard_wrap_word(word: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for grapheme in UnicodeSegmentation::graphemes(word, true) {
        let grapheme_width = display_width(grapheme);
        if current_width > 0 && current_width + grapheme_width > width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(grapheme);
        current_width += grapheme_width;
    }

    lines.push(current);
    lines
}

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn push_segment(segments: &mut TableLine, text: impl Into<String>, kind: TableCellKind) {
    let text = text.into();
    if text.is_empty() {
        return;
    }

    if let Some(last) = segments.last_mut()
        && last.kind == kind
    {
        last.text.push_str(&text);
        return;
    }

    segments.push(TableLineSegment { text, kind });
}
