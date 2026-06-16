use ratatui::text::Line;

use crate::display_width::display_width;

use super::{TerminalPalette, muted_text_style};

const PAGE_RULE_RIGHT_PAD: usize = 2;

/// 构造右侧带 label 的分隔线，宽度按 terminal display width 计算。
pub(crate) fn build_labeled_rule(
    width: u16,
    label: String,
    palette: TerminalPalette,
) -> Line<'static> {
    let width = usize::from(width);
    let label_width = display_width(&label);

    if width <= label_width + PAGE_RULE_RIGHT_PAD {
        return Line::styled(label, muted_text_style(palette));
    }

    let left_dash_count = width.saturating_sub(label_width + PAGE_RULE_RIGHT_PAD);
    let mut line = String::with_capacity(width);
    line.push_str(&"─".repeat(left_dash_count));
    line.push_str(&label);
    line.push_str(&"─".repeat(PAGE_RULE_RIGHT_PAD));

    Line::styled(line, muted_text_style(palette))
}

/// 构造统一的分页分隔线。
pub(crate) fn build_page_rule(
    width: u16,
    page_number: usize,
    page_count: usize,
    palette: TerminalPalette,
) -> Line<'static> {
    let compact_label = format!(" {page_number}/{page_count} ");
    let full_label = format!(" Page {page_number}/{page_count} ");
    let label = if width >= 24 {
        full_label
    } else {
        compact_label
    };
    build_labeled_rule(width, label, palette)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::default_palette;

    #[test]
    fn labeled_rule_uses_display_width_for_wide_labels() {
        let line = build_labeled_rule(12, " 页1/2 ".to_string(), default_palette());
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(display_width(&rendered), 12);
        assert!(rendered.ends_with(" 页1/2 ──"));
    }
}
