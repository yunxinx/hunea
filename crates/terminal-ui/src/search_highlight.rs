//! 跨列表复用的搜索匹配辅助：命中文本高亮与子序列匹配打分。

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

/// 大小写不敏感的子序列匹配分数。lower is better。
///
/// 算法参考 codex-rs `fuzzy_match`：贪心子序列匹配后，以匹配窗口大小作为
/// 基础分数（连续匹配优于离散），首个匹配字符在位置 0 时给前缀奖励，
/// needle 完整覆盖 text 时给完全匹配奖励。
///
/// 返回 `None` 表示不匹配；空 needle 返回 `Some(0)`。
pub(crate) fn subsequence_match_score(text: &str, query: &str) -> Option<i32> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        return Some(0);
    }

    let indices = find_case_insensitive_subsequence_indices(text, trimmed_query)?;

    // 复用高亮路径的同一匹配结果计算分数，确保打分与高亮一致。
    // dedup 处理 Unicode lowercase expansion（如 İ → i̇）导致多个 lowered char
    // 映射回同一 original index 的情况。
    let mut sorted_indices = indices;
    sorted_indices.sort_unstable();
    sorted_indices.dedup();

    let first = *sorted_indices.first()?;
    let last = *sorted_indices.last()?;
    let needle_len = trimmed_query.chars().count() as i32;
    let first_pos = first as i32;
    let last_pos = last as i32;
    let window = (last_pos - first_pos + 1) - needle_len;
    let mut score = window.max(0);

    if first_pos == 0 {
        score -= 100;
    }

    let text_char_count = text.chars().count() as i32;
    // 三个条件合起来等价于 text 与 query 大小写不敏感地完全相等：
    // 首字符命中位置 0（前缀）、末字符命中到 text 尾、去重后命中的索引数
    // 等于 text 字符数（贪心子序列下意味着每个字符都被命中一次）。
    if first_pos == 0
        && last_pos + 1 == text_char_count
        && sorted_indices.len() as i32 == text_char_count
    {
        score -= 1000;
    }

    Some(score)
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

    #[test]
    fn score_prefers_contiguous_match_over_spread() {
        // "resend" 含连续 "se"（window=0），"resume" 含离散 "se"（window=2）
        let resend = subsequence_match_score("resend", "se").expect("resend matches se");
        let resume = subsequence_match_score("resume", "se").expect("resume matches se");
        assert!(resend < resume);
        assert_eq!(resend, 0);
        assert_eq!(resume, 2);
    }

    #[test]
    fn score_rewards_prefix_match() {
        // 前缀匹配（first=0）获得 -100 奖励
        let prefix = subsequence_match_score("resume", "re").expect("prefix match");
        let non_prefix = subsequence_match_score("resume", "su").expect("non-prefix match");
        assert!(prefix < non_prefix);
        assert_eq!(prefix, -100);
        assert_eq!(non_prefix, 0);
    }

    #[test]
    fn score_rewards_exact_match() {
        // 完全匹配获得额外 -1000 奖励
        let exact = subsequence_match_score("resume", "resume").expect("exact match");
        let prefix = subsequence_match_score("resume", "resum").expect("prefix match");
        assert!(exact < prefix);
        assert_eq!(exact, -100 - 1000);
    }

    #[test]
    fn score_case_insensitive() {
        let lower = subsequence_match_score("Resume", "resume").expect("case-insensitive exact");
        assert_eq!(lower, -100 - 1000);
    }

    #[test]
    fn score_returns_none_when_no_match() {
        assert!(subsequence_match_score("resume", "xyz").is_none());
    }

    #[test]
    fn score_empty_query_returns_zero() {
        assert_eq!(subsequence_match_score("resume", ""), Some(0));
        assert_eq!(subsequence_match_score("resume", "   "), Some(0));
    }

    #[test]
    fn score_handles_unicode_lowercase_expansion() {
        // "İ" lowercase 展开为 "i\u{307}"，两形式语义等价，应判为完全匹配
        // （前缀奖励 -100 + 完全匹配奖励 -1000）
        let score = subsequence_match_score("İ", "i\u{307}").expect("unicode expansion match");
        assert_eq!(score, -100 - 1000);
    }
}
