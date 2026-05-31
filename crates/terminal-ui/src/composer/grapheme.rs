use unicode_segmentation::UnicodeSegmentation;

use super::layout::VisualLine;
pub(crate) use crate::display_width::display_width as measure_width;

#[derive(Debug, Clone)]
pub(crate) struct GraphemeCluster<'a> {
    pub(crate) text: &'a str,
    pub(crate) start_char: usize,
    pub(crate) end_char: usize,
    pub(crate) width: usize,
}

pub(crate) fn grapheme_range_before_cursor(
    line: &str,
    cursor_chars: usize,
) -> Option<(usize, usize)> {
    if cursor_chars == 0 {
        return None;
    }

    let cursor_chars = cursor_chars.min(line.chars().count());
    let mut previous = None;

    for cluster in grapheme_clusters(line) {
        if cursor_chars == cluster.start_char {
            return previous
                .map(|previous: GraphemeCluster<'_>| (previous.start_char, previous.end_char));
        }

        if cursor_chars > cluster.start_char && cursor_chars <= cluster.end_char {
            return Some((cluster.start_char, cluster.end_char));
        }

        previous = Some(cluster);
    }

    previous.map(|cluster| (cluster.start_char, cluster.end_char))
}

pub(crate) fn grapheme_range_at_or_after_cursor(
    line: &str,
    cursor_chars: usize,
) -> Option<(usize, usize)> {
    let total_chars = line.chars().count();
    if cursor_chars >= total_chars {
        return None;
    }

    for cluster in grapheme_clusters(line) {
        if cursor_chars <= cluster.start_char {
            return Some((cluster.start_char, cluster.end_char));
        }

        if cursor_chars > cluster.start_char && cursor_chars < cluster.end_char {
            return Some((cluster.start_char, cluster.end_char));
        }
    }

    None
}

pub(crate) fn grapheme_target_left(line: &str, cursor_chars: usize) -> Option<usize> {
    grapheme_range_before_cursor(line, cursor_chars).map(|(start, _)| start)
}

pub(crate) fn grapheme_target_right(line: &str, cursor_chars: usize) -> Option<usize> {
    let total_chars = line.chars().count();
    if cursor_chars >= total_chars {
        return None;
    }

    for cluster in grapheme_clusters(line) {
        if cursor_chars <= cluster.start_char {
            return Some(cluster.end_char);
        }

        if cursor_chars > cluster.start_char && cursor_chars < cluster.end_char {
            return Some(cluster.end_char);
        }
    }

    Some(total_chars)
}

pub(crate) fn logical_column_for_visual_offset(
    line: &VisualLine,
    visual_offset: usize,
    content_width: usize,
) -> usize {
    if visual_offset == 0 {
        return line.visible_start_char;
    }

    if content_width == 0 || line.text.is_empty() {
        return line.visible_start_char;
    }

    if !line.column_offsets.is_empty() {
        for (index, offset) in line.column_offsets.iter().enumerate() {
            if *offset == visual_offset {
                return line.visible_start_char + index;
            }
            if *offset >= visual_offset {
                if index == 0 {
                    return line.visible_start_char;
                }

                let mut boundary_index = index - 1;
                while boundary_index > 0
                    && line.column_offsets[boundary_index - 1]
                        == line.column_offsets[boundary_index]
                {
                    boundary_index -= 1;
                }
                return line.visible_start_char + boundary_index;
            }
        }

        return line.end_char;
    }

    let mut consumed_width = 0;
    let mut consumed_chars = 0;
    for cluster in grapheme_clusters(&line.text) {
        if consumed_width + cluster.width > visual_offset {
            return line.visible_start_char + consumed_chars;
        }

        consumed_width += cluster.width;
        consumed_chars += cluster.end_char - cluster.start_char;
        if consumed_width >= visual_offset {
            return line.visible_start_char + consumed_chars;
        }
    }

    line.visible_start_char + consumed_chars
}

pub(crate) fn is_space_cluster(cluster: &str) -> bool {
    !cluster.is_empty() && cluster.chars().all(char::is_whitespace)
}

pub(crate) fn grapheme_clusters(text: &str) -> Vec<GraphemeCluster<'_>> {
    let mut clusters = Vec::new();
    let mut start_char = 0;

    for cluster in text.graphemes(true) {
        let char_count = cluster.chars().count();
        clusters.push(GraphemeCluster {
            text: cluster,
            start_char,
            end_char: start_char + char_count,
            width: measure_width(cluster),
        });
        start_char += char_count;
    }

    clusters
}
