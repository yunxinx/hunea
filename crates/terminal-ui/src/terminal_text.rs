use std::{borrow::Cow, iter::Peekable};

const ESC: char = '\u{1b}';
const BEL: char = '\u{7}';
const C1_DCS: char = '\u{90}';
const C1_SOS: char = '\u{98}';
const C1_CSI: char = '\u{9b}';
const C1_ST: char = '\u{9c}';
const C1_OSC: char = '\u{9d}';
const C1_PM: char = '\u{9e}';
const C1_APC: char = '\u{9f}';
const STRING_CONTROL_STARTERS: [char; 4] = ['P', 'X', '^', '_'];
const CHARSET_CONTROL_STARTERS: [char; 7] = ['(', ')', '*', '+', '-', '.', '/'];

/// 移除不能直接写入 Ratatui `Line`/`Buffer` 的终端控制序列。
pub(crate) fn sanitize_terminal_text(text: &str) -> Cow<'_, str> {
    if !needs_terminal_text_sanitization(text) {
        return Cow::Borrowed(text);
    }

    let mut sanitized = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            ESC => skip_escape_sequence(&mut characters),
            C1_CSI => skip_csi_sequence(&mut characters),
            C1_OSC | C1_DCS | C1_SOS | C1_PM | C1_APC => {
                skip_string_control_sequence(&mut characters);
            }
            character if character.is_control() && !is_preserved_text_control(character) => {}
            character => sanitized.push(character),
        }
    }

    Cow::Owned(sanitized)
}

fn needs_terminal_text_sanitization(text: &str) -> bool {
    text.chars()
        .any(|character| character == ESC || is_removed_control(character))
}

fn is_removed_control(character: char) -> bool {
    character.is_control() && !is_preserved_text_control(character)
}

fn is_preserved_text_control(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\t')
}

fn skip_escape_sequence<I>(characters: &mut Peekable<I>)
where
    I: Iterator<Item = char>,
{
    match characters.next() {
        Some('[') => skip_csi_sequence(characters),
        Some(']') => skip_string_control_sequence(characters),
        Some(starter) if STRING_CONTROL_STARTERS.contains(&starter) => {
            skip_string_control_sequence(characters);
        }
        Some(starter) if CHARSET_CONTROL_STARTERS.contains(&starter) || starter == '%' => {
            let _ = characters.next();
        }
        Some(_) | None => {}
    }
}

fn skip_string_control_sequence<I>(characters: &mut Peekable<I>)
where
    I: Iterator<Item = char>,
{
    while let Some(character) = characters.next() {
        if character == BEL || character == C1_ST {
            break;
        }

        if character == ESC && characters.peek() == Some(&'\\') {
            let _ = characters.next();
            break;
        }
    }
}

fn skip_csi_sequence<I>(characters: &mut Peekable<I>)
where
    I: Iterator<Item = char>,
{
    for character in characters.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&character) {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::sanitize_terminal_text;

    #[test]
    fn sanitize_terminal_text_borrows_clean_text() {
        assert!(matches!(
            sanitize_terminal_text("hello 2️⃣"),
            Cow::Borrowed("hello 2️⃣")
        ));
    }

    #[test]
    fn sanitize_terminal_text_removes_csi_sequences() {
        assert_eq!(
            sanitize_terminal_text("a\u{1b}[31mb\u{1b}[0m").as_ref(),
            "ab"
        );
    }

    #[test]
    fn sanitize_terminal_text_removes_osc_bel_sequences() {
        assert_eq!(
            sanitize_terminal_text("a\u{1b}]8;;https://example.test\u{7}b").as_ref(),
            "ab"
        );
    }

    #[test]
    fn sanitize_terminal_text_removes_osc_st_sequences() {
        assert_eq!(
            sanitize_terminal_text("a\u{1b}]8;;https://example.test\u{1b}\\b").as_ref(),
            "ab"
        );
    }

    #[test]
    fn sanitize_terminal_text_preserves_text_layout_controls() {
        assert_eq!(sanitize_terminal_text("a\r\nb\tc").as_ref(), "a\r\nb\tc");
    }

    #[test]
    fn sanitize_terminal_text_removes_other_controls() {
        assert_eq!(sanitize_terminal_text("a\u{0}b\u{7f}c").as_ref(), "abc");
    }

    #[test]
    fn sanitize_terminal_text_removes_c1_string_controls() {
        assert_eq!(
            sanitize_terminal_text("a\u{9d}ignored\u{9c}b").as_ref(),
            "ab"
        );
    }
}
