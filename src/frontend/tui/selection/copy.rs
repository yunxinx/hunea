use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::frontend::tui::document::{DocumentAnchorRegion, DocumentLayout};

use super::{SelectionState, selection_columns_for_line, selection_ends_before_line_content};

pub(crate) fn selection_text(layout: &DocumentLayout, selection: SelectionState) -> Option<String> {
    let (start, end) = selection.ordered_points()?;
    if start.line() >= layout.line_count() || end.line() >= layout.line_count() {
        return None;
    }

    let mut lines = Vec::with_capacity(end.line().saturating_sub(start.line()) + 1);
    for line in start.line()..=end.line() {
        if let Some(line_data) = layout.line_at(line)
            && let Some((start_column, end_column)) =
                selection_columns_for_line(selection, line, line_data.selectable)
        {
            lines.push(selection_text_for_line(
                &line_data.plain_line,
                start_column,
                end_column,
            ));
            continue;
        }

        let line_data = layout.line_at(line);
        let preserves_blank = line_data
            .as_ref()
            .is_some_and(|line_data| line_preserves_blank_selection(&line_data.anchor));
        let selectable = line_data
            .map(|line_data| line_data.selectable)
            .unwrap_or_default();
        if preserves_blank || selection_ends_before_line_content(selection, line, selectable) {
            lines.push(String::new());
        }
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(crate) fn selection_text_for_line(
    text: &str,
    start_column: usize,
    end_column: usize,
) -> String {
    if start_column >= end_column {
        return String::new();
    }

    let mut rendered = String::new();
    let mut column = 0;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if width == 0 {
            continue;
        }
        let cluster_start = column;
        let cluster_end = column + width;
        column = cluster_end;
        if cluster_end <= start_column || cluster_start >= end_column {
            continue;
        }
        rendered.push_str(grapheme);
    }

    rendered
}

fn line_preserves_blank_selection(
    anchor: &crate::frontend::tui::document::DocumentLineAnchor,
) -> bool {
    match anchor.region {
        DocumentAnchorRegion::Transcript => !matches!(
            anchor.transcript.item_anchor.kind,
            crate::frontend::tui::transcript::LineAnchorKind::ItemGap
        ),
        DocumentAnchorRegion::Composer => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::document::DocumentLayout;

    #[test]
    fn selection_text_for_line_keeps_visible_graphemes_only() {
        assert_eq!(selection_text_for_line("中a", 0, 2), "中");
        assert_eq!(selection_text_for_line("hello", 1, 4), "ell");
    }

    #[test]
    fn out_of_range_selection_is_ignored() {
        let layout = DocumentLayout::default();
        let mut selection = SelectionState::default();
        selection.select_range(
            super::super::SelectionPoint::new(1, 0),
            super::super::SelectionPoint::new(2, 1),
        );

        assert_eq!(selection_text(&layout, selection), None);
    }
}
