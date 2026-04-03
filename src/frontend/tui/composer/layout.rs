use super::grapheme::{grapheme_clusters, is_space_cluster, measure_width};

#[derive(Debug, Clone)]
struct WrapSegment {
    text: String,
    width: usize,
    start_char: usize,
    char_count: usize,
    is_space: bool,
}

#[derive(Debug, Clone)]
struct WrappedLine {
    text: String,
    start_char: usize,
    end_char: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct VisualLine {
    pub(crate) text: String,
    pub(crate) logical_line: usize,
    pub(crate) start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) is_continuation: bool,
}

pub(crate) fn visual_line_count(value: &str, width: usize) -> usize {
    visual_lines_for_text(value, width).len().max(1)
}

pub(crate) fn placeholder_line_count(value: &str, width: usize) -> usize {
    placeholder_visual_lines_for_text(value, width).len().max(1)
}

pub(crate) fn visual_lines_for_text(text: &str, width: usize) -> Vec<VisualLine> {
    visual_lines_for_text_with_options(text, width, true)
}

pub(crate) fn placeholder_visual_lines_for_text(text: &str, width: usize) -> Vec<VisualLine> {
    visual_lines_for_text_with_options(text, width, false)
}

fn visual_lines_for_text_with_options(
    text: &str,
    width: usize,
    expand_overflow_spaces: bool,
) -> Vec<VisualLine> {
    let mut lines = Vec::new();

    for (logical_line, line) in text.split('\n').enumerate() {
        let mut wrapped_lines = wrap_detailed(line, width);
        if expand_overflow_spaces {
            wrapped_lines = expand_overflow_wrapped_lines(wrapped_lines, width);
        }

        for (visual_index, wrapped_line) in wrapped_lines.into_iter().enumerate() {
            lines.push(VisualLine {
                text: wrapped_line.text,
                logical_line,
                start_char: wrapped_line.start_char,
                end_char: wrapped_line.end_char,
                is_continuation: visual_index > 0,
            });
        }
    }

    if lines.is_empty() {
        lines.push(VisualLine {
            text: String::new(),
            logical_line: 0,
            start_char: 0,
            end_char: 0,
            is_continuation: false,
        });
    }

    lines
}

fn expand_overflow_wrapped_lines(lines: Vec<WrappedLine>, width: usize) -> Vec<WrappedLine> {
    if width == 0 || lines.is_empty() {
        return lines;
    }

    let mut expanded = Vec::with_capacity(lines.len());

    for line in lines {
        if measure_width(&line.text) <= width {
            expanded.push(line);
            continue;
        }

        let line_width = measure_width(&line.text);
        let segment = WrapSegment {
            text: line.text,
            width: line_width,
            start_char: line.start_char,
            char_count: line.end_char.saturating_sub(line.start_char),
            is_space: false,
        };

        for part in hard_wrap_segment(segment, width) {
            expanded.push(WrappedLine {
                start_char: part.start_char,
                end_char: part.start_char + part.char_count,
                text: part.text,
            });
        }
    }

    expanded
}

fn wrap_detailed(text: &str, width: usize) -> Vec<WrappedLine> {
    if width == 0 {
        return vec![WrappedLine {
            text: text.to_string(),
            start_char: 0,
            end_char: text.chars().count(),
        }];
    }

    if text.is_empty() {
        return vec![WrappedLine {
            text: String::new(),
            start_char: 0,
            end_char: 0,
        }];
    }

    let segments = tokenize_wrap_segments(text);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut current_start_char = 0;
    let mut current_char_count = 0;

    let flush_current = |lines: &mut Vec<WrappedLine>,
                         current: &mut String,
                         current_width: &mut usize,
                         current_start_char: usize,
                         current_char_count: &mut usize| {
        lines.push(WrappedLine {
            text: std::mem::take(current),
            start_char: current_start_char,
            end_char: current_start_char + *current_char_count,
        });
        *current_width = 0;
        *current_char_count = 0;
    };

    for segment in segments {
        if segment.width <= width {
            if current_char_count == 0 || current_width + segment.width <= width {
                if current_char_count == 0 {
                    current_start_char = segment.start_char;
                }
                current.push_str(&segment.text);
                current_width += segment.width;
                current_char_count += segment.char_count;
                continue;
            }

            if segment.is_space && current_char_count > 0 {
                current.push_str(&segment.text);
                current_width += segment.width;
                current_char_count += segment.char_count;
                flush_current(
                    &mut lines,
                    &mut current,
                    &mut current_width,
                    current_start_char,
                    &mut current_char_count,
                );
                continue;
            }

            flush_current(
                &mut lines,
                &mut current,
                &mut current_width,
                current_start_char,
                &mut current_char_count,
            );
            current_start_char = segment.start_char;
            current.push_str(&segment.text);
            current_width += segment.width;
            current_char_count += segment.char_count;
            continue;
        }

        if current_char_count > 0 {
            flush_current(
                &mut lines,
                &mut current,
                &mut current_width,
                current_start_char,
                &mut current_char_count,
            );
        }

        for part in hard_wrap_segment(segment, width) {
            current_start_char = part.start_char;
            current.push_str(&part.text);
            current_width += part.width;
            current_char_count += part.char_count;
            if current_width >= width {
                flush_current(
                    &mut lines,
                    &mut current,
                    &mut current_width,
                    current_start_char,
                    &mut current_char_count,
                );
            }
        }
    }

    if current_char_count > 0 {
        flush_current(
            &mut lines,
            &mut current,
            &mut current_width,
            current_start_char,
            &mut current_char_count,
        );
    }

    if lines.is_empty() {
        lines.push(WrappedLine {
            text: String::new(),
            start_char: 0,
            end_char: 0,
        });
    }

    lines
}

fn tokenize_wrap_segments(text: &str) -> Vec<WrapSegment> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut current_chars = 0;
    let mut current_start_char = 0;
    let mut current_is_space = false;
    let mut has_current = false;
    let mut chars_consumed = 0;

    let flush_current = |segments: &mut Vec<WrapSegment>,
                         current: &mut String,
                         current_width: &mut usize,
                         current_chars: &mut usize,
                         current_start_char: usize,
                         current_is_space: bool,
                         has_current: &mut bool| {
        if !*has_current {
            return;
        }

        segments.push(WrapSegment {
            text: std::mem::take(current),
            width: *current_width,
            start_char: current_start_char,
            char_count: *current_chars,
            is_space: current_is_space,
        });
        *current_width = 0;
        *current_chars = 0;
        *has_current = false;
    };

    for cluster in grapheme_clusters(text) {
        let cluster_is_space = is_space_cluster(cluster.text);
        if !has_current {
            current_start_char = chars_consumed;
            current_is_space = cluster_is_space;
            has_current = true;
        } else if current_is_space != cluster_is_space {
            flush_current(
                &mut segments,
                &mut current,
                &mut current_width,
                &mut current_chars,
                current_start_char,
                current_is_space,
                &mut has_current,
            );
            current_start_char = chars_consumed;
            current_is_space = cluster_is_space;
            has_current = true;
        }

        current.push_str(cluster.text);
        current_width += cluster.width;
        current_chars += cluster.end_char - cluster.start_char;
        chars_consumed += cluster.end_char - cluster.start_char;
    }

    flush_current(
        &mut segments,
        &mut current,
        &mut current_width,
        &mut current_chars,
        current_start_char,
        current_is_space,
        &mut has_current,
    );
    segments
}

fn hard_wrap_segment(segment: WrapSegment, width: usize) -> Vec<WrapSegment> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;
    let mut current_chars = 0;
    let mut current_start_char = segment.start_char;
    let mut chars_consumed = 0;

    let flush_current = |parts: &mut Vec<WrapSegment>,
                         current: &mut String,
                         current_width: &mut usize,
                         current_chars: &mut usize,
                         current_start_char: &mut usize,
                         chars_consumed: usize| {
        if *current_chars == 0 {
            return;
        }

        parts.push(WrapSegment {
            text: std::mem::take(current),
            width: *current_width,
            start_char: *current_start_char,
            char_count: *current_chars,
            is_space: false,
        });
        *current_width = 0;
        *current_chars = 0;
        *current_start_char += chars_consumed;
    };

    for cluster in grapheme_clusters(&segment.text) {
        if current_width + cluster.width > width && current_chars > 0 {
            flush_current(
                &mut parts,
                &mut current,
                &mut current_width,
                &mut current_chars,
                &mut current_start_char,
                chars_consumed,
            );
            chars_consumed = 0;
        }

        current.push_str(cluster.text);
        current_width += cluster.width;
        let char_count = cluster.end_char - cluster.start_char;
        current_chars += char_count;
        chars_consumed += char_count;
    }

    flush_current(
        &mut parts,
        &mut current,
        &mut current_width,
        &mut current_chars,
        &mut current_start_char,
        chars_consumed,
    );
    parts
}
