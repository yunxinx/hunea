use crate::frontend::tui::transcript::wrap_prompt_visual_lines;

#[derive(Debug, Clone)]
pub(crate) struct VisualLine {
    pub(crate) text: String,
    pub(crate) logical_line: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) column_offsets: Vec<usize>,
    pub(crate) is_continuation: bool,
}

pub(crate) fn visual_line_count(value: &str, width: usize, line_prefix_width: usize) -> usize {
    visual_lines_for_text(value, width, line_prefix_width)
        .len()
        .max(1)
}

#[cfg(test)]
pub(crate) fn placeholder_line_count(value: &str, width: usize, line_prefix_width: usize) -> usize {
    placeholder_visual_lines_for_text(value, width, line_prefix_width)
        .len()
        .max(1)
}

pub(crate) fn visual_lines_for_text(
    text: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<VisualLine> {
    visual_lines_for_text_with_options(text, width, line_prefix_width)
}

pub(crate) fn placeholder_visual_lines_for_text(
    text: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<VisualLine> {
    visual_lines_for_text_with_options(text, width, line_prefix_width)
}

fn visual_lines_for_text_with_options(
    text: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<VisualLine> {
    let wrapped_lines = wrap_prompt_visual_lines(text, width, line_prefix_width);
    let mut lines = Vec::with_capacity(wrapped_lines.len());

    for wrapped_line in wrapped_lines {
        let is_continuation = lines
            .last()
            .map(|line: &VisualLine| line.logical_line == wrapped_line.logical_line)
            .unwrap_or(false);
        lines.push(VisualLine {
            text: wrapped_line.text,
            logical_line: wrapped_line.logical_line,
            visible_start_char: wrapped_line.visible_start_char,
            end_char: wrapped_line.end_char,
            column_offsets: wrapped_line.column_offsets,
            is_continuation,
        });
    }

    if lines.is_empty() {
        lines.push(VisualLine {
            text: String::new(),
            logical_line: 0,
            visible_start_char: 0,
            end_char: 0,
            column_offsets: Vec::new(),
            is_continuation: false,
        });
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::{placeholder_line_count, visual_line_count};

    #[test]
    fn visual_line_count_returns_one_for_empty_or_non_positive_width() {
        assert_eq!(visual_line_count("", 10, 2), 1);
        assert_eq!(visual_line_count("hello", 0, 2), 1);
    }

    #[test]
    fn visual_line_count_counts_wrapped_prompt_lines() {
        assert_eq!(visual_line_count("hello world", 7, 2), 2);
    }

    #[test]
    fn placeholder_line_count_counts_wrapped_placeholder_lines() {
        assert_eq!(placeholder_line_count("hello world", 7, 2), 2);
        assert_eq!(placeholder_line_count("", 7, 2), 1);
    }
}
