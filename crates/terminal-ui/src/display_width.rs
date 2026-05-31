use ratatui::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 返回终端可见文本占用的显示列宽。
///
/// 调用方必须传入已经过 TUI 文本净化的可见文本；这个模块只负责宽度语义，
/// 不解析 ANSI/OSC 等跨字符控制序列。
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// 返回单个 grapheme cluster 的终端显示列宽。
pub(crate) fn grapheme_width(grapheme: &str) -> usize {
    display_width(grapheme)
}

/// 返回单个字符的终端显示列宽。
pub(crate) fn char_display_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// 返回 Ratatui `Line` 的终端显示列宽。
pub(crate) fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| display_width(span.content.as_ref()))
        .sum()
}

#[cfg(test)]
mod tests {
    use ratatui::text::{Line, Span};

    use super::*;

    #[test]
    fn display_width_counts_keycap_as_wide_grapheme() {
        assert_eq!(display_width("2️⃣"), 2);
    }

    #[test]
    fn line_display_width_sums_visible_span_content() {
        let line = Line::from(vec![Span::raw("试"), Span::raw("2️⃣")]);

        assert_eq!(line_display_width(&line), 4);
    }
}
