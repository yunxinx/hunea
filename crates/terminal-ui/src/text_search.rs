//! 跨 picker 复用的文本搜索辅助（大小写不敏感子串）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 可复用的大小写不敏感查询。
pub(crate) struct CaseInsensitiveQuery<'a> {
    needle: &'a str,
    folded_unicode: Option<FoldedUnicodeNeedle>,
}

struct FoldedUnicodeNeedle {
    chars: Vec<char>,
    prefix_lengths: Vec<usize>,
}

impl<'a> CaseInsensitiveQuery<'a> {
    /// 构造查询；非 ASCII needle 会预先 lowercase，避免每行重复计算。
    pub(crate) fn new(needle: &'a str) -> Self {
        let folded_unicode = (!needle.is_ascii()).then(|| {
            let chars: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
            let prefix_lengths = kmp_prefix_lengths(&chars);
            FoldedUnicodeNeedle {
                chars,
                prefix_lengths,
            }
        });

        Self {
            needle,
            folded_unicode,
        }
    }

    /// 判断 haystack 是否包含当前查询。
    pub(crate) fn matches(&self, haystack: &str) -> bool {
        if self.needle.is_empty() {
            return true;
        }

        if let Some(folded_needle) = &self.folded_unicode {
            return contains_folded_unicode(haystack, folded_needle);
        }

        let needle_bytes = self.needle.as_bytes();
        haystack
            .as_bytes()
            .windows(needle_bytes.len())
            .any(|window| window.eq_ignore_ascii_case(needle_bytes))
    }

    /// 判断 haystack 是否以当前查询为前缀。
    pub(crate) fn starts_with(&self, haystack: &str) -> bool {
        if self.needle.is_empty() {
            return true;
        }

        if let Some(folded_needle) = &self.folded_unicode {
            let mut haystack_chars = haystack.chars().flat_map(char::to_lowercase);
            return folded_needle.chars.iter().all(|needle| {
                haystack_chars
                    .next()
                    .is_some_and(|haystack| haystack == *needle)
            });
        }

        let needle_bytes = self.needle.as_bytes();
        haystack
            .as_bytes()
            .get(..needle_bytes.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(needle_bytes))
    }
}

fn contains_folded_unicode(haystack: &str, needle: &FoldedUnicodeNeedle) -> bool {
    if needle.chars.is_empty() {
        return true;
    }

    let mut matched = 0usize;
    for character in haystack.chars().flat_map(char::to_lowercase) {
        while matched > 0 && needle.chars[matched] != character {
            matched = needle.prefix_lengths[matched - 1];
        }

        if needle.chars[matched] == character {
            matched += 1;
            if matched == needle.chars.len() {
                return true;
            }
        }
    }

    false
}

fn kmp_prefix_lengths(needle: &[char]) -> Vec<usize> {
    let mut prefix_lengths = vec![0; needle.len()];
    let mut matched = 0usize;

    for index in 1..needle.len() {
        while matched > 0 && needle[matched] != needle[index] {
            matched = prefix_lengths[matched - 1];
        }
        if needle[matched] == needle[index] {
            matched += 1;
            prefix_lengths[index] = matched;
        }
    }

    prefix_lengths
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
    fn case_insensitive_query_ascii() {
        let world = CaseInsensitiveQuery::new("world");
        let hello = CaseInsensitiveQuery::new("HELLO");
        let xyz = CaseInsensitiveQuery::new("xyz");
        assert!(world.matches("Hello World"));
        assert!(hello.matches("Hello World"));
        assert!(!xyz.matches("Hello World"));
        assert!(hello.starts_with("hello world"));
        assert!(!world.starts_with("Hello World"));
    }

    #[test]
    fn empty_needle_matches_all() {
        let empty = CaseInsensitiveQuery::new("");
        assert!(empty.matches("anything"));
    }

    #[test]
    fn reusable_query_matches_unicode_without_ascii_fallback() {
        let query = CaseInsensitiveQuery::new("ÄF");

        assert!(query.matches("die äffin ruft"));
        assert!(!query.matches("die affe ruft"));
        assert!(query.starts_with("äffin"));
        assert!(!query.starts_with("affe"));
    }
}
