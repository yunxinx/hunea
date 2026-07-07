//! 跨列表复用的搜索命中文本高亮。

use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// 为命中文本生成与项目现有 inline code 类似的背景强调。
pub(crate) fn search_match_style(base_style: Style, surface: Option<Color>) -> Style {
    match surface {
        Some(surface) => base_style.bg(surface),
        None => base_style.reversed(),
    }
}

/// 对大小写不敏感的连续子串命中做高亮。
pub(crate) fn highlighted_substring_spans(
    text: &str,
    query: &str,
    base_style: Style,
    highlighted_style: Style,
) -> Vec<Span<'static>> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let Some((match_start, match_end)) = find_case_insensitive_substring(text, trimmed_query)
    else {
        return vec![Span::styled(text.to_string(), base_style)];
    };

    highlighted_char_ranges_spans(
        text,
        &[(match_start, match_end)],
        base_style,
        highlighted_style,
    )
}

/// 对大小写不敏感的离散子序列命中做高亮。
pub(crate) fn highlighted_subsequence_spans(
    text: &str,
    query: &str,
    base_style: Style,
    highlighted_style: Style,
) -> Vec<Span<'static>> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let Some(match_indices) = find_case_insensitive_subsequence_indices(text, trimmed_query) else {
        return vec![Span::styled(text.to_string(), base_style)];
    };

    let match_ranges = contiguous_ranges(&match_indices);
    highlighted_char_ranges_spans(text, &match_ranges, base_style, highlighted_style)
}

/// 优先高亮连续子串；若仅是模糊/子序列命中，则退回到离散字符高亮。
pub(crate) fn highlighted_substring_or_subsequence_spans(
    text: &str,
    query: &str,
    base_style: Style,
    highlighted_style: Style,
) -> Vec<Span<'static>> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    if let Some((match_start, match_end)) = find_case_insensitive_substring(text, trimmed_query) {
        return highlighted_char_ranges_spans(
            text,
            &[(match_start, match_end)],
            base_style,
            highlighted_style,
        );
    }

    highlighted_subsequence_spans(text, trimmed_query, base_style, highlighted_style)
}

fn highlighted_char_ranges_spans(
    text: &str,
    match_ranges: &[(usize, usize)],
    base_style: Style,
    highlighted_style: Style,
) -> Vec<Span<'static>> {
    if match_ranges.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let mut spans = Vec::new();
    let mut cursor = 0usize;
    let text_char_len = text.chars().count();

    for &(start, end) in match_ranges {
        if cursor < start {
            spans.push(Span::styled(
                slice_char_range(text, cursor, start),
                base_style,
            ));
        }
        spans.push(Span::styled(
            slice_char_range(text, start, end),
            highlighted_style,
        ));
        cursor = end;
    }

    if cursor < text_char_len {
        spans.push(Span::styled(
            slice_char_range(text, cursor, text_char_len),
            base_style,
        ));
    }

    spans
}

fn contiguous_ranges(indices: &[usize]) -> Vec<(usize, usize)> {
    let Some(&first_index) = indices.first() else {
        return Vec::new();
    };

    let mut ranges = Vec::new();
    let mut range_start = first_index;
    let mut previous = first_index;

    for &index in indices.iter().skip(1) {
        if index == previous + 1 {
            previous = index;
            continue;
        }

        ranges.push((range_start, previous + 1));
        range_start = index;
        previous = index;
    }

    ranges.push((range_start, previous + 1));
    ranges
}

fn find_case_insensitive_subsequence_indices(text: &str, query: &str) -> Option<Vec<usize>> {
    let query_chars = fold_chars(query);
    if query_chars.is_empty() {
        return Some(Vec::new());
    }

    let mut query_index = 0usize;
    let mut matched_indices = Vec::new();
    for (text_index, character) in text.chars().enumerate() {
        let folded_character = fold_char(character);
        let next_query_index = query_index + folded_character.len();
        if next_query_index > query_chars.len() {
            continue;
        }
        if query_chars[query_index..next_query_index] == folded_character {
            matched_indices.push(text_index);
            query_index = next_query_index;
            if query_index == query_chars.len() {
                return Some(matched_indices);
            }
        }
    }

    None
}

fn find_case_insensitive_substring(text: &str, query: &str) -> Option<(usize, usize)> {
    let query_chars = fold_chars(query);
    if query_chars.is_empty() {
        return None;
    }

    let text_chars = text.chars().collect::<Vec<_>>();
    let mut folded_text = Vec::new();
    let mut folded_to_original = Vec::new();
    for (index, character) in text_chars.iter().copied().enumerate() {
        for lowered in character.to_lowercase() {
            folded_text.push(lowered);
            folded_to_original.push(index);
        }
    }

    if query_chars.len() > folded_text.len() {
        return None;
    }

    for start in 0..=folded_text.len() - query_chars.len() {
        if folded_text[start..start + query_chars.len()] == query_chars {
            let start_index = folded_to_original[start];
            let end_index = folded_to_original[start + query_chars.len() - 1] + 1;
            return Some((start_index, end_index));
        }
    }

    None
}

fn fold_chars(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).collect()
}

fn fold_char(character: char) -> Vec<char> {
    character.to_lowercase().collect()
}

fn slice_char_range(text: &str, start: usize, end: usize) -> String {
    text.chars()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::*;

    #[test]
    fn substring_highlight_splits_unicode_match_by_original_char_range() {
        let spans = highlighted_substring_spans(
            "İstanbul capable",
            "i\u{307}sta",
            Style::new(),
            Style::new().reversed(),
        );

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>(),
            vec!["İsta", "nbul capable"]
        );
        assert!(
            spans[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }

    #[test]
    fn subsequence_highlight_groups_only_adjacent_match_runs() {
        let spans =
            highlighted_subsequence_spans("/models", "md", Style::new(), Style::new().reversed());

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>(),
            vec!["/", "m", "o", "d", "els"]
        );
    }
}
