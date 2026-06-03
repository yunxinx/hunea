use super::grapheme::grapheme_clusters;
use super::{Composer, LineAnchor, absolute_cursor_for_position, logical_lines};
use crate::display_width::display_width;

/// `cursor_position_for_line_anchor_click` 把 composer 某条视觉行上的点击换算为逻辑光标位置。
pub(crate) fn cursor_position_for_line_anchor_click(
    composer: &Composer,
    anchor: LineAnchor,
    mouse_x: usize,
) -> Option<(usize, usize)> {
    if composer.value().is_empty() {
        return None;
    }

    let prompt_width = display_width(composer.prompt());
    let visual_offset = if mouse_x < prompt_width {
        0
    } else {
        mouse_x - prompt_width + 1
    };
    let line_text = visual_text_for_anchor(composer.value(), anchor)?;
    let logical_column = logical_column_for_visual_click(&line_text, anchor, visual_offset);

    Some((anchor.logical_line, logical_column))
}

/// `selection_start_char_for_line_anchor` 把 composer 选区起点列换算为绝对字符偏移。
pub(crate) fn selection_start_char_for_line_anchor(
    composer: &Composer,
    anchor: LineAnchor,
    column: usize,
) -> Option<usize> {
    selection_char_for_line_anchor(composer, anchor, column, SelectionBoundary::Start)
}

/// `selection_end_char_for_line_anchor` 把 composer 选区终点列换算为绝对字符偏移。
pub(crate) fn selection_end_char_for_line_anchor(
    composer: &Composer,
    anchor: LineAnchor,
    column: usize,
) -> Option<usize> {
    selection_char_for_line_anchor(composer, anchor, column, SelectionBoundary::End)
}

fn visual_text_for_anchor(value: &str, anchor: LineAnchor) -> Option<String> {
    let lines = logical_lines(value);
    let line = lines.get(anchor.logical_line)?;
    if anchor.visible_start_char > anchor.end_char || anchor.visible_start_char > line.len_chars() {
        return None;
    }

    let end_char = anchor.end_char.min(line.len_chars());
    Some(
        line.text
            .chars()
            .skip(anchor.visible_start_char)
            .take(end_char.saturating_sub(anchor.visible_start_char))
            .collect(),
    )
}

fn logical_column_for_visual_click(
    line_text: &str,
    anchor: LineAnchor,
    visual_offset: usize,
) -> usize {
    if visual_offset == 0 || line_text.is_empty() {
        return anchor.visible_start_char;
    }

    let mut current_width = 0;
    let mut consumed_chars = 0;
    for cluster in grapheme_clusters(line_text) {
        let cluster_chars = cluster.end_char.saturating_sub(cluster.start_char);
        if cluster.width == 0 {
            consumed_chars += cluster_chars;
            continue;
        }

        current_width += cluster.width;
        consumed_chars += cluster_chars;
        if visual_offset <= current_width {
            return anchor.visible_start_char + consumed_chars;
        }
    }

    anchor.end_char
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionBoundary {
    Start,
    End,
}

fn selection_char_for_line_anchor(
    composer: &Composer,
    anchor: LineAnchor,
    column: usize,
    boundary: SelectionBoundary,
) -> Option<usize> {
    if composer.value().is_empty() {
        return None;
    }

    let visual_offset = column.saturating_sub(display_width(composer.prompt()));
    let line_text = visual_text_for_anchor(composer.value(), anchor)?;
    let logical_column =
        logical_column_for_selection_boundary(&line_text, anchor, visual_offset, boundary);
    Some(absolute_cursor_for_position(
        &logical_lines(composer.value()),
        anchor.logical_line,
        logical_column,
    ))
}

fn logical_column_for_selection_boundary(
    line_text: &str,
    anchor: LineAnchor,
    visual_offset: usize,
    boundary: SelectionBoundary,
) -> usize {
    if visual_offset == 0 || line_text.is_empty() {
        return anchor.visible_start_char;
    }

    let mut current_width = 0;
    let mut consumed_chars = 0;
    for cluster in grapheme_clusters(line_text) {
        let cluster_chars = cluster.end_char.saturating_sub(cluster.start_char);
        let cluster_start_width = current_width;
        let cluster_end_width = current_width + cluster.width;

        match boundary {
            SelectionBoundary::Start if cluster_end_width > visual_offset => {
                return anchor.visible_start_char + consumed_chars;
            }
            SelectionBoundary::End if cluster_start_width >= visual_offset => {
                return anchor.visible_start_char + consumed_chars;
            }
            _ => {}
        }

        current_width = cluster_end_width;
        consumed_chars += cluster_chars;
    }

    anchor.end_char
}

/// `move_cursor_to_logical_position` 直接把 composer 光标移动到目标逻辑行列。
pub(crate) fn move_cursor_to_logical_position(
    composer: &mut Composer,
    logical_line: usize,
    logical_column: usize,
) {
    composer.finish_current_undo_group();
    let lines = logical_lines(composer.value());
    composer.set_cursor(absolute_cursor_for_position(
        &lines,
        logical_line,
        logical_column,
    ));
}
