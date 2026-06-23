//! 跨 picker 复用的文本搜索辅助（大小写不敏感子串）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 在 `haystack` 中查找 `needle` 的子串（大小写不敏感）。
pub(crate) fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.is_ascii() {
        let needle_bytes = needle.as_bytes();
        return haystack
            .as_bytes()
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes));
    }

    let needle_lower = needle.to_lowercase();
    haystack
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .contains(&needle_lower)
}

/// 搜索模式下可追加到 query 的可打印字符键（与 session picker 一致）。
pub(crate) fn is_picker_search_text_key(key: &KeyEvent) -> bool {
    let KeyCode::Char(character) = key.code else {
        return false;
    };
    !character.is_ascii_control()
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_case_insensitive_ascii() {
        assert!(contains_case_insensitive("Hello World", "world"));
        assert!(contains_case_insensitive("Hello World", "HELLO"));
        assert!(!contains_case_insensitive("Hello World", "xyz"));
    }

    #[test]
    fn empty_needle_matches_all() {
        assert!(contains_case_insensitive("anything", ""));
    }
}
