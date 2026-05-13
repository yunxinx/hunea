use ratatui::{
    style::Modifier,
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub(crate) fn apply_selection_to_line(
    line: &Line<'static>,
    start_column: usize,
    end_column: usize,
) -> Line<'static> {
    if start_column >= end_column {
        return line.clone();
    }

    let mut spans = Vec::new();
    let mut column = 0;
    for span in &line.spans {
        let base_style = line.style.patch(span.style);
        for grapheme in span.content.as_ref().graphemes(true) {
            let width = grapheme.width();
            let cluster_start = column;
            let cluster_end = column + width;
            if width > 0 {
                column = cluster_end;
            }

            let selected = width > 0 && cluster_start < end_column && cluster_end > start_column;
            let style = if selected {
                base_style.add_modifier(Modifier::REVERSED)
            } else {
                base_style
            };
            push_span(&mut spans, style, grapheme);
        }
    }

    let mut selected_line = Line::from(spans);
    selected_line.alignment = line.alignment;
    selected_line
}

fn push_span(spans: &mut Vec<Span<'static>>, style: ratatui::style::Style, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(last) = spans.last_mut()
        && last.style == style
    {
        last.content = format!("{}{}", last.content, text).into();
        return;
    }

    spans.push(Span::styled(text.to_string(), style));
}

#[cfg(test)]
mod tests {
    use ratatui::{
        style::{Color, Modifier, Style},
        text::Line,
    };

    use super::apply_selection_to_line;

    #[test]
    fn selection_render_marks_only_selected_clusters_as_reversed() {
        let line = Line::styled("hello", Style::default().fg(Color::Red));
        let rendered = apply_selection_to_line(&line, 1, 4);

        assert_eq!(
            rendered
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "hello"
        );
        assert!(
            !rendered.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            rendered.spans[1]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !rendered.spans[2]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn selection_render_preserves_line_level_style_for_unselected_clusters() {
        let line = Line::styled("hello", Style::default().fg(Color::Blue));
        let rendered = apply_selection_to_line(&line, 1, 2);

        assert_eq!(rendered.spans[0].style.fg, Some(Color::Blue));
        assert_eq!(rendered.spans[2].style.fg, Some(Color::Blue));
        assert!(
            !rendered.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !rendered.spans[2]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn selection_render_patches_line_and_span_styles_before_reversing() {
        let mut line = Line::from(vec![
            ratatui::text::Span::styled("a", Style::default().bg(Color::Red)),
            ratatui::text::Span::raw("b"),
        ]);
        line.style = Style::default().fg(Color::Blue);

        let rendered = apply_selection_to_line(&line, 0, 1);

        assert_eq!(rendered.spans[0].style.fg, Some(Color::Blue));
        assert_eq!(rendered.spans[0].style.bg, Some(Color::Red));
        assert!(
            rendered.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert_eq!(rendered.spans[1].style.fg, Some(Color::Blue));
        assert!(
            !rendered.spans[1]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn selection_render_uses_display_width_for_wide_graphemes() {
        let line = Line::raw("中a");
        let rendered = apply_selection_to_line(&line, 0, 2);

        assert!(
            rendered.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !rendered.spans[1]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }
}
