use super::{grapheme::measure_width, layout::VisualLine};

#[cfg(test)]
pub(crate) fn visible_viewport_lines<T>(
    lines: &[T],
    offset: usize,
    height: usize,
) -> (&[T], usize) {
    if lines.is_empty() {
        return (&[], 0);
    }

    if height == 0 || height >= lines.len() {
        return (lines, 0);
    }

    let max_offset = lines.len().saturating_sub(height);
    let offset = offset.min(max_offset);
    (&lines[offset..offset + height], offset)
}

pub(crate) fn calculate_cursor_visual_position(
    lines: &[VisualLine],
    logical_line: usize,
    logical_column: usize,
    prompt_width: usize,
) -> (usize, usize) {
    let Some((first_line, last_line)) = logical_line_bounds(lines, logical_line) else {
        return (0, prompt_width);
    };

    if logical_column == 0 {
        return (first_line, prompt_width);
    }

    let last_visual_line = &lines[last_line];
    let logical_column = logical_column.min(last_visual_line.end_char);

    for (line_index, line) in lines
        .iter()
        .enumerate()
        .take(last_line + 1)
        .skip(first_line)
    {
        if logical_column == line.end_char && line_index < last_line {
            let next_line = &lines[line_index + 1];
            if next_line.visible_start_char <= logical_column
                && logical_column <= next_line.end_char
            {
                continue;
            }
            if next_line.visible_start_char == logical_column {
                return (line_index + 1, prompt_width);
            }
        }

        if logical_column > line.end_char {
            continue;
        }

        if logical_column <= line.visible_start_char {
            return (line_index, prompt_width);
        }

        return (
            line_index,
            prompt_width + visual_offset_for_logical_column(line, logical_column),
        );
    }

    (
        last_line,
        prompt_width + measure_width(&last_visual_line.text),
    )
}

pub(crate) fn sync_viewport_offset_for_cursor(
    current_offset: usize,
    viewport_height: usize,
    total_lines: usize,
    cursor_visual_y: usize,
) -> usize {
    if total_lines == 0 {
        return 0;
    }

    let mut current_offset = clamp_viewport_offset(current_offset, viewport_height, total_lines);
    if cursor_visual_y < current_offset {
        current_offset = cursor_visual_y;
    } else if cursor_visual_y >= current_offset.saturating_add(viewport_height.max(1)) {
        current_offset = cursor_visual_y.saturating_sub(viewport_height.max(1) - 1);
    }

    clamp_viewport_offset(current_offset, viewport_height, total_lines)
}

fn clamp_viewport_offset(offset: usize, viewport_height: usize, total_lines: usize) -> usize {
    if total_lines == 0 || viewport_height == 0 {
        return 0;
    }

    offset.min(total_lines.saturating_sub(viewport_height))
}

fn logical_line_bounds(lines: &[VisualLine], logical_line: usize) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;

    for (index, line) in lines.iter().enumerate() {
        if line.logical_line != logical_line {
            if first.is_some() {
                break;
            }
            continue;
        }

        first.get_or_insert(index);
        last = Some(index);
    }

    match (first, last) {
        (Some(first), Some(last)) => Some((first, last)),
        _ => None,
    }
}

fn visual_offset_for_logical_column(line: &VisualLine, logical_column: usize) -> usize {
    if logical_column <= line.visible_start_char {
        return 0;
    }
    if logical_column >= line.end_char {
        return line
            .column_offsets
            .last()
            .copied()
            .unwrap_or_else(|| measure_width(&line.text));
    }

    let index = logical_column.saturating_sub(line.visible_start_char);
    if index < line.column_offsets.len() {
        return line.column_offsets[index];
    }

    measure_width(&line.text)
}
