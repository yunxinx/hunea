use std::cmp::Ordering;

/// `natural_sort_text_cmp` 以更贴近人类直觉的方式比较文本中的数字片段。
#[must_use]
pub fn natural_sort_text_cmp(left: &str, right: &str) -> Ordering {
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (Some(left_char), Some(right_char))
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() =>
            {
                let left_number = take_ascii_digit_run(&mut left_chars);
                let right_number = take_ascii_digit_run(&mut right_chars);
                let digit_cmp = left_number
                    .trim_start_matches('0')
                    .len()
                    .cmp(&right_number.trim_start_matches('0').len());
                if digit_cmp != Ordering::Equal {
                    return digit_cmp;
                }
                let value_cmp = left_number
                    .trim_start_matches('0')
                    .cmp(right_number.trim_start_matches('0'));
                if value_cmp != Ordering::Equal {
                    return value_cmp;
                }
                let zero_padding_cmp = left_number.len().cmp(&right_number.len());
                if zero_padding_cmp != Ordering::Equal {
                    return zero_padding_cmp;
                }
            }
            (Some(left_char), Some(right_char)) => {
                let normalized_left = left_char.to_ascii_lowercase();
                let normalized_right = right_char.to_ascii_lowercase();
                let char_cmp = normalized_left.cmp(&normalized_right);
                if char_cmp != Ordering::Equal {
                    return char_cmp;
                }
                left_chars.next();
                right_chars.next();
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

fn take_ascii_digit_run<I>(chars: &mut std::iter::Peekable<I>) -> String
where
    I: Iterator<Item = char>,
{
    let mut digits = String::new();
    while let Some(character) = chars.peek().copied() {
        if !character.is_ascii_digit() {
            break;
        }
        digits.push(character);
        chars.next();
    }
    digits
}
