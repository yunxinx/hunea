#[cfg(test)]
thread_local! {
    static COMPOSER_VISUAL_LINES_CALL_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

use std::rc::Rc;

use crate::transcript::wrap_prompt_visual_lines;

/// `ComposerLayoutKey` 唯一标识一次 composer 全文 soft-wrap 的输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ComposerLayoutKey {
    pub(crate) content_revision: usize,
    pub(crate) content_width: usize,
    pub(crate) prompt_width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VisualLine {
    pub(crate) text: String,
    pub(crate) logical_line: usize,
    pub(crate) logical_line_start_char: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) is_continuation: bool,
}

/// `ComposerLayoutSnapshot` 保存当前 content/width revision 的不可变视觉几何。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerLayoutSnapshot {
    key: ComposerLayoutKey,
    visual_lines: Vec<VisualLine>,
}

impl ComposerLayoutSnapshot {
    pub(crate) fn build(
        value: &str,
        content_revision: usize,
        content_width: usize,
        prompt_width: usize,
    ) -> Rc<Self> {
        Rc::new(Self {
            key: ComposerLayoutKey {
                content_revision,
                content_width,
                prompt_width,
            },
            visual_lines: visual_lines_for_text(value, content_width, prompt_width),
        })
    }

    pub(crate) fn visual_lines(&self) -> &[VisualLine] {
        &self.visual_lines
    }

    pub(crate) fn line_count(&self) -> usize {
        self.visual_lines.len().max(1)
    }
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
    #[cfg(test)]
    COMPOSER_VISUAL_LINES_CALL_COUNT.with(|count| count.set(count.get() + 1));

    visual_lines_for_text_with_options(text, width, line_prefix_width)
}

pub(crate) fn placeholder_visual_lines_for_text(
    text: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<VisualLine> {
    visual_lines_for_text_with_options(text, width, line_prefix_width)
}

#[cfg(test)]
pub(crate) fn reset_visual_lines_call_count() {
    COMPOSER_VISUAL_LINES_CALL_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn visual_lines_call_count() -> usize {
    COMPOSER_VISUAL_LINES_CALL_COUNT.with(std::cell::Cell::get)
}

fn visual_lines_for_text_with_options(
    text: &str,
    width: usize,
    line_prefix_width: usize,
) -> Vec<VisualLine> {
    let wrapped_lines = wrap_prompt_visual_lines(text, width, line_prefix_width);
    let logical_line_start_chars = logical_line_start_chars(text);
    let mut lines = Vec::with_capacity(wrapped_lines.len());

    for wrapped_line in wrapped_lines {
        let is_continuation = lines
            .last()
            .map(|line: &VisualLine| line.logical_line == wrapped_line.logical_line)
            .unwrap_or(false);
        lines.push(VisualLine {
            text: wrapped_line.text,
            logical_line: wrapped_line.logical_line,
            logical_line_start_char: logical_line_start_chars
                .get(wrapped_line.logical_line)
                .copied()
                .unwrap_or(0),
            visible_start_char: wrapped_line.visible_start_char,
            end_char: wrapped_line.end_char,
            is_continuation,
        });
    }

    if lines.is_empty() {
        lines.push(VisualLine {
            text: String::new(),
            logical_line: 0,
            logical_line_start_char: 0,
            visible_start_char: 0,
            end_char: 0,
            is_continuation: false,
        });
    }

    lines
}

fn logical_line_start_chars(text: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut start_char = 0usize;

    for line in text.split('\n') {
        starts.push(start_char);
        start_char += line.chars().count() + 1;
    }

    if starts.is_empty() {
        starts.push(0);
    }

    starts
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
