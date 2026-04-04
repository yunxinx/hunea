use super::grapheme::grapheme_clusters;
use super::{
    Composer, LineAnchor, absolute_cursor_for_position, layout::visual_lines_for_text,
    logical_lines,
};
use unicode_width::UnicodeWidthStr;

/// `cursor_position_for_line_anchor_click` 把 composer 某条视觉行上的点击换算为逻辑光标位置。
pub(crate) fn cursor_position_for_line_anchor_click(
    composer: &Composer,
    anchor: LineAnchor,
    mouse_x: usize,
) -> Option<(usize, usize)> {
    if composer.value().is_empty() {
        return None;
    }

    let visual_lines = visual_lines_for_text(
        composer.value(),
        composer.content_width(),
        composer.prompt().width(),
    );
    let line = visual_lines.iter().find(|line| {
        line.logical_line == anchor.logical_line
            && line.visible_start_char == anchor.visible_start_char
            && line.end_char == anchor.end_char
    })?;
    let prompt_width = composer.prompt().width();
    let visual_offset = if mouse_x < prompt_width {
        0
    } else {
        mouse_x - prompt_width + 1
    };
    let logical_column = logical_column_for_visual_click(line, visual_offset);

    Some((anchor.logical_line, logical_column))
}

fn logical_column_for_visual_click(
    line: &super::layout::VisualLine,
    visual_offset: usize,
) -> usize {
    if visual_offset == 0 || line.text.is_empty() {
        return line.visible_start_char;
    }

    let mut current_width = 0;
    let mut consumed_chars = 0;
    for cluster in grapheme_clusters(&line.text) {
        let cluster_chars = cluster.end_char.saturating_sub(cluster.start_char);
        if cluster.width == 0 {
            consumed_chars += cluster_chars;
            continue;
        }

        current_width += cluster.width;
        consumed_chars += cluster_chars;
        if visual_offset <= current_width {
            return line.visible_start_char + consumed_chars;
        }
    }

    line.end_char
}

/// `move_cursor_to_logical_position` 直接把 composer 光标移动到目标逻辑行列。
pub(crate) fn move_cursor_to_logical_position(
    composer: &mut Composer,
    logical_line: usize,
    logical_column: usize,
) {
    let lines = logical_lines(composer.value());
    composer.set_cursor(absolute_cursor_for_position(
        &lines,
        logical_line,
        logical_column,
    ));
    composer.sync_viewport_to_cursor();
}
