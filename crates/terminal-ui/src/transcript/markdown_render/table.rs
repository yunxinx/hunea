use pulldown_cmark::Alignment;
use ratatui::style::Style;

use super::wrapping::{
    LogicalLine, StyledChunk, WrapMode, chunk_width, measure_width, push_chunk,
    wrap_styled_chunks_for_width,
};

const TABLE_COLUMN_GAP: usize = 2;
const TABLE_CELL_PADDING: usize = 1;
const TABLE_HEADER_SEPARATOR_CHAR: char = '━';
const TABLE_BODY_SEPARATOR_CHAR: char = '─';
const MIN_COLUMN_WIDTH: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TableCell {
    lines: Vec<Vec<StyledChunk>>,
}

impl TableCell {
    pub(super) fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }

        for segment in text.split_inclusive('\n') {
            let mut line_text = segment.strip_suffix('\n').unwrap_or(segment);
            if let Some(stripped) = line_text.strip_suffix('\r') {
                line_text = stripped;
            }
            self.ensure_line();
            if let Some(line) = self.lines.last_mut() {
                push_chunk(line, line_text, style);
            }
            if segment.ends_with('\n') {
                self.hard_break();
            }
        }
    }

    pub(super) fn push_space_if_needed(&mut self, style: Style) {
        self.ensure_line();
        let Some(line) = self.lines.last_mut() else {
            return;
        };
        if line.is_empty() || line_ends_with_whitespace(line) {
            return;
        }
        push_chunk(line, " ", style);
    }

    pub(super) fn hard_break(&mut self) {
        self.lines.push(Vec::new());
    }

    fn ensure_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(Vec::new());
        }
    }

    fn plain_text(&self) -> String {
        let mut text = String::new();
        for (line_index, line) in self.lines.iter().enumerate() {
            if line_index > 0 {
                text.push(' ');
            }
            for chunk in line {
                text.push_str(&chunk.text);
            }
        }
        text
    }

    fn display_width(&self) -> usize {
        self.lines
            .iter()
            .map(|line| chunk_width(line))
            .max()
            .unwrap_or(0)
    }

    fn wrapped_lines(&self, width: usize, row_style: Style) -> Vec<Vec<StyledChunk>> {
        if self.lines.is_empty() {
            return vec![Vec::new()];
        }

        let mut wrapped = Vec::new();
        for line in &self.lines {
            if line.is_empty() {
                wrapped.push(Vec::new());
                continue;
            }
            wrapped.extend(
                wrap_styled_chunks_for_width(line, width.max(1))
                    .into_iter()
                    .map(|chunks| patch_chunks(chunks, row_style)),
            );
        }
        if wrapped.is_empty() {
            wrapped.push(Vec::new());
        }
        wrapped
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TableBodyRow {
    cells: Vec<TableCell>,
    has_table_pipe_syntax: bool,
}

impl TableBodyRow {
    pub(super) fn new(cells: Vec<TableCell>, has_table_pipe_syntax: bool) -> Self {
        Self {
            cells,
            has_table_pipe_syntax,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MarkdownTable {
    pub(super) alignments: Vec<Alignment>,
    pub(super) header: Vec<TableCell>,
    pub(super) rows: Vec<TableBodyRow>,
}

#[derive(Clone, Debug)]
pub(super) struct TableRenderOptions {
    pub(super) width: usize,
    pub(super) first_prefix: Vec<StyledChunk>,
    pub(super) continuation_prefix: Vec<StyledChunk>,
    pub(super) header_style: Style,
    pub(super) body_style: Style,
    pub(super) separator_style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableColumnKind {
    Narrative,
    TokenHeavy,
    Compact,
}

#[derive(Clone, Debug)]
struct TableColumnMetrics {
    max_width: usize,
    header_token_width: usize,
    body_token_width: usize,
    kind: TableColumnKind,
}

pub(super) fn render_markdown_table(
    mut table: MarkdownTable,
    options: TableRenderOptions,
) -> Vec<LogicalLine> {
    let column_count = table_column_count(&table);
    if column_count == 0 {
        return Vec::new();
    }

    let mut spillover_cells = Vec::new();
    let mut rows = Vec::with_capacity(table.rows.len());
    for (row_index, row) in table.rows.iter().enumerate() {
        let next_row = table.rows.get(row_index + 1);
        if column_count > 1 && is_spillover_row(row, next_row) {
            if let Some(cell) = row.cells.first() {
                spillover_cells.push(cell.clone());
            }
        } else {
            rows.push(row.cells.clone());
        }
    }

    let mut header = if table.header.is_empty() {
        vec![TableCell::default(); column_count]
    } else {
        std::mem::take(&mut table.header)
    };
    normalize_row(&mut header, column_count);
    for row in &mut rows {
        normalize_row(row, column_count);
    }

    let metrics = collect_table_column_metrics(&header, &rows, column_count);
    let column_widths = compute_column_widths(
        &metrics,
        column_count,
        available_table_width(column_count, &options),
    );
    let mut rendered = if let Some(widths) = column_widths {
        if should_render_records(&rows, &widths, &metrics) {
            render_records(&header, &rows, &metrics, &options)
        } else {
            render_grid(&header, &rows, &widths, &table.alignments, &options)
        }
    } else if !rows.is_empty() {
        render_records(&header, &rows, &metrics, &options)
    } else {
        render_pipe_fallback(&header, &table.alignments, &options)
    };

    for cell in spillover_cells {
        rendered.extend(render_spillover_cell(&cell, &options));
    }

    rendered
}

fn table_column_count(table: &MarkdownTable) -> usize {
    table.alignments.len().max(table.header.len()).max(
        table
            .rows
            .iter()
            .map(|row| row.cells.len())
            .max()
            .unwrap_or(0),
    )
}

fn normalize_row(row: &mut Vec<TableCell>, column_count: usize) {
    row.truncate(column_count);
    row.resize(column_count, TableCell::default());
}

fn available_content_width(options: &TableRenderOptions) -> usize {
    let prefix_width =
        chunk_width(&options.first_prefix).max(chunk_width(&options.continuation_prefix));
    options.width.saturating_sub(prefix_width).max(1)
}

fn available_table_width(column_count: usize, options: &TableRenderOptions) -> usize {
    let reserved = column_count.saturating_mul(TABLE_CELL_PADDING * 2)
        + column_count
            .saturating_sub(1)
            .saturating_mul(TABLE_COLUMN_GAP);
    available_content_width(options).saturating_sub(reserved)
}

fn render_grid(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    column_widths: &[usize],
    alignments: &[Alignment],
    options: &TableRenderOptions,
) -> Vec<LogicalLine> {
    let mut output = Vec::with_capacity(rows.len().saturating_mul(2).saturating_add(2));
    output.extend(
        render_table_row(header, column_widths, alignments, options.header_style)
            .into_iter()
            .map(|chunks| table_logical_line(chunks, options, WrapMode::Literal)),
    );
    output.push(table_logical_line(
        render_separator_chunks(
            column_widths,
            TABLE_HEADER_SEPARATOR_CHAR,
            options.separator_style,
        ),
        options,
        WrapMode::Literal,
    ));

    for (row_index, row) in rows.iter().enumerate() {
        output.extend(
            render_table_row(row, column_widths, alignments, options.body_style)
                .into_iter()
                .map(|chunks| table_logical_line(chunks, options, WrapMode::Literal)),
        );
        if row_index + 1 < rows.len() {
            output.push(table_logical_line(
                render_separator_chunks(
                    column_widths,
                    TABLE_BODY_SEPARATOR_CHAR,
                    options.separator_style,
                ),
                options,
                WrapMode::Literal,
            ));
        }
    }

    output
}

fn render_table_row(
    row: &[TableCell],
    column_widths: &[usize],
    alignments: &[Alignment],
    row_style: Style,
) -> Vec<Vec<StyledChunk>> {
    let wrapped_cells = row
        .iter()
        .zip(column_widths.iter())
        .map(|(cell, width)| cell.wrapped_lines(*width, row_style))
        .collect::<Vec<_>>();
    let row_height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
    let mut rendered = Vec::with_capacity(row_height);

    for visual_line_index in 0..row_height {
        let Some(last_visible_column) = wrapped_cells.iter().rposition(|cell_lines| {
            cell_lines
                .get(visual_line_index)
                .is_some_and(|chunks| chunk_width(chunks) > 0)
        }) else {
            rendered.push(Vec::new());
            continue;
        };

        let mut chunks = Vec::new();
        for (column_index, width) in column_widths
            .iter()
            .copied()
            .enumerate()
            .take(last_visible_column + 1)
        {
            push_chunk(&mut chunks, " ".repeat(TABLE_CELL_PADDING), row_style);
            let cell_chunks = wrapped_cells[column_index]
                .get(visual_line_index)
                .cloned()
                .unwrap_or_default();
            let cell_width = chunk_width(&cell_chunks);
            let remaining = width.saturating_sub(cell_width);
            let (left_padding, right_padding) =
                alignment_padding(alignment_for_column(alignments, column_index), remaining);
            if left_padding > 0 {
                push_chunk(&mut chunks, " ".repeat(left_padding), row_style);
            }
            chunks.extend(cell_chunks);
            let is_last_column = column_index == last_visible_column;
            if right_padding > 0 && !is_last_column {
                push_chunk(&mut chunks, " ".repeat(right_padding), row_style);
            }
            if !is_last_column {
                push_chunk(&mut chunks, " ".repeat(TABLE_CELL_PADDING), row_style);
                push_chunk(&mut chunks, " ".repeat(TABLE_COLUMN_GAP), row_style);
            }
        }
        rendered.push(chunks);
    }

    rendered
}

fn render_separator_chunks(
    column_widths: &[usize],
    separator_char: char,
    style: Style,
) -> Vec<StyledChunk> {
    let mut chunks = Vec::new();
    for (index, width) in column_widths.iter().enumerate() {
        if index > 0 {
            push_chunk(&mut chunks, " ".repeat(TABLE_COLUMN_GAP), style);
        }
        push_chunk(
            &mut chunks,
            separator_char
                .to_string()
                .repeat(width + TABLE_CELL_PADDING * 2),
            style,
        );
    }
    chunks
}

fn render_records(
    headers: &[TableCell],
    rows: &[Vec<TableCell>],
    metrics: &[TableColumnMetrics],
    options: &TableRenderOptions,
) -> Vec<LogicalLine> {
    let label_width = headers
        .iter()
        .map(|header| measure_width(&header.plain_text()))
        .max()
        .unwrap_or(0);
    let has_expansive_value = metrics
        .iter()
        .any(|metrics| metrics.kind != TableColumnKind::Compact);
    let minimum_value_width = if has_expansive_value { 24 } else { 12 };
    let content_width = available_content_width(options);
    let aligned_fields = 1 + label_width + 2 + minimum_value_width <= content_width;
    let mut output = Vec::new();

    for (row_index, row) in rows.iter().enumerate() {
        for (header, value) in headers.iter().zip(row) {
            if aligned_fields {
                render_aligned_record_field(&mut output, header, value, label_width, options);
            } else {
                render_stacked_record_field(&mut output, header, value, options);
            }
        }
        if row_index + 1 < rows.len() {
            output.push(table_logical_line(
                vec![StyledChunk {
                    text: TABLE_BODY_SEPARATOR_CHAR.to_string().repeat(content_width),
                    style: options.separator_style,
                }],
                options,
                WrapMode::Literal,
            ));
        }
    }

    output
}

fn render_aligned_record_field(
    output: &mut Vec<LogicalLine>,
    header: &TableCell,
    value: &TableCell,
    label_width: usize,
    options: &TableRenderOptions,
) {
    let value_indent = 1 + label_width + 2;
    let value_width = available_content_width(options)
        .saturating_sub(value_indent)
        .max(3);
    let wrapped_value = value.wrapped_lines(value_width, options.body_style);

    for (line_index, value_chunks) in wrapped_value.into_iter().enumerate() {
        let mut chunks = Vec::new();
        if line_index == 0 {
            let label = header.plain_text();
            push_chunk(&mut chunks, " ", options.header_style);
            push_chunk(&mut chunks, label.clone(), options.header_style);
            push_chunk(
                &mut chunks,
                " ".repeat(label_width.saturating_sub(measure_width(&label)) + 2),
                options.header_style,
            );
        } else {
            push_chunk(&mut chunks, " ".repeat(value_indent), options.body_style);
        }
        chunks.extend(value_chunks);
        output.push(table_logical_line(chunks, options, WrapMode::Literal));
    }
}

fn render_stacked_record_field(
    output: &mut Vec<LogicalLine>,
    header: &TableCell,
    value: &TableCell,
    options: &TableRenderOptions,
) {
    let label_width = available_content_width(options).saturating_sub(1).max(1);
    let label_chunks = vec![StyledChunk {
        text: header.plain_text(),
        style: options.header_style,
    }];
    for label_line in wrap_styled_chunks_for_width(&label_chunks, label_width) {
        let mut chunks = vec![StyledChunk {
            text: " ".to_string(),
            style: options.header_style,
        }];
        chunks.extend(label_line);
        output.push(table_logical_line(chunks, options, WrapMode::Literal));
    }

    let value_width = available_content_width(options).saturating_sub(2).max(1);
    for value_line in value.wrapped_lines(value_width, options.body_style) {
        let mut chunks = vec![StyledChunk {
            text: "  ".to_string(),
            style: options.body_style,
        }];
        chunks.extend(value_line);
        output.push(table_logical_line(chunks, options, WrapMode::Literal));
    }
}

fn render_pipe_fallback(
    header: &[TableCell],
    alignments: &[Alignment],
    options: &TableRenderOptions,
) -> Vec<LogicalLine> {
    vec![
        table_logical_line(
            row_to_pipe_chunks(header, options.body_style),
            options,
            WrapMode::Prose,
        ),
        table_logical_line(
            vec![StyledChunk {
                text: alignments_to_pipe_delimiter(alignments),
                style: options.body_style,
            }],
            options,
            WrapMode::Prose,
        ),
    ]
}

fn row_to_pipe_chunks(row: &[TableCell], style: Style) -> Vec<StyledChunk> {
    let mut chunks = vec![StyledChunk {
        text: "|".to_string(),
        style,
    }];
    for cell in row {
        push_chunk(&mut chunks, " ", style);
        push_chunk(&mut chunks, escape_table_pipes(&cell.plain_text()), style);
        push_chunk(&mut chunks, " |", style);
    }
    chunks
}

fn alignments_to_pipe_delimiter(alignments: &[Alignment]) -> String {
    let mut output = String::from("|");
    for alignment in alignments {
        output.push_str(match alignment {
            Alignment::Left => ":---",
            Alignment::Center => ":---:",
            Alignment::Right => "---:",
            Alignment::None => "---",
        });
        output.push('|');
    }
    output
}

fn render_spillover_cell(cell: &TableCell, options: &TableRenderOptions) -> Vec<LogicalLine> {
    cell.lines
        .iter()
        .cloned()
        .map(|chunks| {
            table_logical_line(
                patch_chunks(chunks, options.body_style),
                options,
                WrapMode::Prose,
            )
        })
        .collect()
}

fn table_logical_line(
    chunks: Vec<StyledChunk>,
    options: &TableRenderOptions,
    wrap_mode: WrapMode,
) -> LogicalLine {
    LogicalLine {
        first_prefix: options.first_prefix.clone(),
        continuation_prefix: options.continuation_prefix.clone(),
        chunks,
        wrap_mode,
        preserve_trailing_spaces: false,
    }
}

fn compute_column_widths(
    metrics: &[TableColumnMetrics],
    column_count: usize,
    available_width: usize,
) -> Option<Vec<usize>> {
    let mut widths = metrics
        .iter()
        .map(|column| column.max_width.max(MIN_COLUMN_WIDTH))
        .collect::<Vec<_>>();

    let minimum_total = column_count.saturating_mul(MIN_COLUMN_WIDTH);
    if available_width < minimum_total {
        return None;
    }

    let mut floors = metrics
        .iter()
        .map(preferred_column_floor)
        .collect::<Vec<_>>();
    let mut floor_total = floors.iter().sum::<usize>();
    while floor_total > available_width {
        let Some((index, _)) = floors
            .iter()
            .enumerate()
            .filter(|(_, floor)| **floor > MIN_COLUMN_WIDTH)
            .min_by_key(|(index, floor)| {
                (
                    column_shrink_priority(metrics[*index].kind),
                    usize::MAX.saturating_sub(**floor),
                )
            })
        else {
            break;
        };
        floors[index] -= 1;
        floor_total -= 1;
    }

    let mut total_width = widths.iter().sum::<usize>();
    while total_width > available_width {
        let Some(index) = next_column_to_shrink(&widths, &floors, metrics) else {
            break;
        };
        widths[index] -= 1;
        total_width -= 1;
    }

    (total_width <= available_width).then_some(widths)
}

fn collect_table_column_metrics(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    column_count: usize,
) -> Vec<TableColumnMetrics> {
    let mut metrics = Vec::with_capacity(column_count);
    for column_index in 0..column_count {
        let header_cell = &header[column_index];
        let header_text = header_cell.plain_text();
        let header_token_width = longest_token_width(&header_text);
        let mut max_width = header_cell.display_width();
        let mut body_token_width = 0;
        let mut body_token_count = 0usize;
        let mut long_body_token_count = 0usize;
        let mut total_words = 0usize;
        let mut total_cells = 0usize;
        let mut total_cell_width = 0usize;

        for row in rows {
            let cell = &row[column_index];
            max_width = max_width.max(cell.display_width());
            let plain = cell.plain_text();
            body_token_width = body_token_width.max(longest_token_width(&plain));
            let word_count = plain.split_whitespace().count();
            if word_count > 0 {
                body_token_count += word_count;
                long_body_token_count += plain
                    .split_whitespace()
                    .filter(|token| measure_width(token) >= 20)
                    .count();
                total_words += word_count;
                total_cells += 1;
                total_cell_width += measure_width(&plain);
            }
        }

        let avg_words_per_cell = if total_cells == 0 {
            header_text.split_whitespace().count() as f64
        } else {
            total_words as f64 / total_cells as f64
        };
        let avg_cell_width = if total_cells == 0 {
            measure_width(&header_text) as f64
        } else {
            total_cell_width as f64 / total_cells as f64
        };
        let kind = if long_body_token_count > 0
            && long_body_token_count >= body_token_count.saturating_sub(long_body_token_count)
        {
            TableColumnKind::TokenHeavy
        } else if avg_words_per_cell >= 4.0 || avg_cell_width >= 28.0 {
            TableColumnKind::Narrative
        } else {
            TableColumnKind::Compact
        };

        metrics.push(TableColumnMetrics {
            max_width,
            header_token_width,
            body_token_width,
            kind,
        });
    }

    metrics
}

fn preferred_column_floor(metrics: &TableColumnMetrics) -> usize {
    let target = match metrics.kind {
        TableColumnKind::Narrative | TableColumnKind::TokenHeavy => 16,
        TableColumnKind::Compact => metrics
            .header_token_width
            .max(metrics.body_token_width.min(16)),
    };
    target.max(MIN_COLUMN_WIDTH).min(metrics.max_width)
}

fn next_column_to_shrink(
    widths: &[usize],
    floors: &[usize],
    metrics: &[TableColumnMetrics],
) -> Option<usize> {
    widths
        .iter()
        .enumerate()
        .filter(|(index, width)| **width > floors[*index])
        .min_by_key(|(index, width)| {
            (
                column_shrink_priority(metrics[*index].kind),
                usize::MAX.saturating_sub(width.saturating_sub(floors[*index])),
            )
        })
        .map(|(index, _)| index)
}

fn column_shrink_priority(kind: TableColumnKind) -> usize {
    match kind {
        TableColumnKind::TokenHeavy => 0,
        TableColumnKind::Narrative => 1,
        TableColumnKind::Compact => 2,
    }
}

fn should_render_records(
    rows: &[Vec<TableCell>],
    column_widths: &[usize],
    metrics: &[TableColumnMetrics],
) -> bool {
    if rows.is_empty() {
        return false;
    }

    let affected_rows = rows
        .iter()
        .filter(|row| {
            let contains_fragmented_value =
                row.iter()
                    .zip(column_widths)
                    .zip(metrics)
                    .any(|((cell, width), metrics)| {
                        let has_fragmented_token = cell
                            .plain_text()
                            .split_whitespace()
                            .any(|token| measure_width(token) > *width);
                        match metrics.kind {
                            TableColumnKind::Compact => has_fragmented_token,
                            TableColumnKind::TokenHeavy => *width < 12 && has_fragmented_token,
                            TableColumnKind::Narrative => false,
                        }
                    });

            contains_fragmented_value || expansive_cells_are_starved(row, column_widths, metrics)
        })
        .count();
    let threshold = if rows.len() == 1 {
        1
    } else {
        2.max(rows.len().div_ceil(3))
    };

    affected_rows >= threshold
}

fn expansive_cells_are_starved(
    row: &[TableCell],
    column_widths: &[usize],
    metrics: &[TableColumnMetrics],
) -> bool {
    let expansive_cells = row
        .iter()
        .zip(column_widths)
        .zip(metrics)
        .filter(|&((_cell, _width), metrics)| metrics.kind != TableColumnKind::Compact)
        .map(|((cell, width), metrics)| {
            (
                metrics.kind,
                *width,
                cell.wrapped_lines(*width, Style::new()).len(),
            )
        })
        .collect::<Vec<_>>();

    expansive_cells
        .iter()
        .filter(|(_, _, height)| *height >= 4)
        .count()
        >= 2
        || expansive_cells.iter().any(|(kind, width, height)| {
            *kind == TableColumnKind::Narrative && *width < 12 && *height >= 7
        })
}

fn is_spillover_row(row: &TableBodyRow, next_row: Option<&TableBodyRow>) -> bool {
    let Some(first_text) = first_non_empty_only_text(&row.cells) else {
        return false;
    };

    if row.cells.len() == 1 && !row.has_table_pipe_syntax {
        return true;
    }

    if looks_like_html_content(&first_text) {
        return true;
    }

    if first_text.trim_end().ends_with(':') {
        if next_row
            .and_then(|row| first_non_empty_only_text(&row.cells))
            .is_some_and(|text| looks_like_html_content(&text))
        {
            return true;
        }

        if next_row.is_none() && looks_like_html_label_line(&first_text) {
            return true;
        }
    }

    false
}

fn first_non_empty_only_text(row: &[TableCell]) -> Option<String> {
    let first = row.first()?.plain_text();
    if first.trim().is_empty() {
        return None;
    }
    let rest_empty = row[1..]
        .iter()
        .all(|cell| cell.plain_text().trim().is_empty());
    rest_empty.then_some(first)
}

fn looks_like_html_content(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (index, &byte) in bytes.iter().enumerate() {
        if byte != b'<' {
            continue;
        }

        let mut tag_start = index + 1;
        if tag_start < bytes.len() && matches!(bytes[tag_start], b'/' | b'!') {
            tag_start += 1;
        }

        if bytes.get(tag_start).is_some_and(u8::is_ascii_alphabetic)
            && bytes
                .get(tag_start + 1..)
                .is_some_and(|suffix| suffix.contains(&b'>'))
        {
            return true;
        }
    }
    false
}

fn looks_like_html_label_line(text: &str) -> bool {
    let trimmed = text.trim();
    if !trimmed.ends_with(':') {
        return false;
    }

    let label = trimmed.trim_end_matches(':').trim();
    label
        .split_whitespace()
        .any(|word| word.eq_ignore_ascii_case("html"))
}

fn alignment_for_column(alignments: &[Alignment], column_index: usize) -> Alignment {
    alignments
        .get(column_index)
        .copied()
        .unwrap_or(Alignment::None)
}

fn alignment_padding(alignment: Alignment, remaining: usize) -> (usize, usize) {
    match alignment {
        Alignment::Right => (remaining, 0),
        Alignment::Center => (remaining / 2, remaining - remaining / 2),
        Alignment::Left | Alignment::None => (0, remaining),
    }
}

fn longest_token_width(text: &str) -> usize {
    text.split_whitespace()
        .map(measure_width)
        .max()
        .unwrap_or(0)
}

fn patch_chunks(chunks: Vec<StyledChunk>, style: Style) -> Vec<StyledChunk> {
    if style == Style::new() {
        return chunks;
    }

    chunks
        .into_iter()
        .map(|chunk| StyledChunk {
            text: chunk.text,
            style: style.patch(chunk.style),
        })
        .collect()
}

fn line_ends_with_whitespace(line: &[StyledChunk]) -> bool {
    line.last()
        .and_then(|chunk| chunk.text.chars().next_back())
        .is_some_and(char::is_whitespace)
}

fn escape_table_pipes(text: &str) -> String {
    text.replace('|', "\\|")
}
