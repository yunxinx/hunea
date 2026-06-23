//! 两档相对时间标签（`y`…`s`）及固定列宽排版；message history 列表与 entry tree branch picker 共用。

use crate::display_width::display_width;

/// Message history 列表时间列显示宽度。
pub(crate) const RELATIVE_AGE_LIST_COLUMN_WIDTH: usize = 9;
/// Entry tree branch picker Created/Updated 列显示宽度（设计为 7）。
pub(crate) const RELATIVE_AGE_BRANCH_PICKER_TIME_COLUMN_WIDTH: usize = 7;
/// Message history：`·` 前段对齐用的显示列宽。
pub(crate) const RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH: usize = 4;
/// Branch picker（7 列内）：`·` 前段对齐用的显示列宽。
pub(crate) const RELATIVE_AGE_BRANCH_PICKER_BEFORE_DOT_WIDTH: usize = 3;
/// `·` 后数字段的固定显示列宽（双列），使末位单位（`s`/`m` 等）纵向对齐；无额外空格。
const RELATIVE_AGE_SECOND_NUMBER_DISPLAY_WIDTH: usize = 2;

/// 将 `timestamp_ms` 相对 `now_ms` 格式化为紧凑英文标签，例如 `2h·05m`、`3d·02h`。
pub(crate) fn relative_age_label(now_ms: i64, timestamp_ms: i64) -> String {
    const SECONDS_PER_MINUTE: i64 = 60;
    const MINUTES_PER_HOUR: i64 = 60;
    const HOURS_PER_DAY: i64 = 24;
    const DAYS_PER_MONTH: i64 = 30;
    const DAYS_PER_YEAR: i64 = 365;

    if timestamp_ms <= 0 || now_ms <= 0 {
        return "—".to_string();
    }

    let elapsed_seconds = now_ms.saturating_sub(timestamp_ms).max(0) / 1_000;
    if elapsed_seconds < 1 {
        return "now".to_string();
    }

    let elapsed_minutes = elapsed_seconds / SECONDS_PER_MINUTE;
    let elapsed_hours = elapsed_minutes / MINUTES_PER_HOUR;
    let elapsed_days = elapsed_hours / HOURS_PER_DAY;

    let years = elapsed_days / DAYS_PER_YEAR;
    let remaining_days_after_years = elapsed_days % DAYS_PER_YEAR;
    let months = remaining_days_after_years / DAYS_PER_MONTH;
    let days = remaining_days_after_years % DAYS_PER_MONTH;
    let hours = elapsed_hours % HOURS_PER_DAY;
    let minutes = elapsed_minutes % MINUTES_PER_HOUR;
    let seconds = elapsed_seconds % SECONDS_PER_MINUTE;

    two_highest_relative_age_units([
        RelativeAgeUnit {
            value: years,
            suffix: "y",
        },
        RelativeAgeUnit {
            value: months,
            suffix: "mo",
        },
        RelativeAgeUnit {
            value: days,
            suffix: "d",
        },
        RelativeAgeUnit {
            value: hours,
            suffix: "h",
        },
        RelativeAgeUnit {
            value: minutes,
            suffix: "m",
        },
        RelativeAgeUnit {
            value: seconds,
            suffix: "s",
        },
    ])
}

#[derive(Debug, Clone, Copy)]
struct RelativeAgeUnit {
    value: i64,
    suffix: &'static str,
}

fn two_highest_relative_age_units(units: [RelativeAgeUnit; 6]) -> String {
    let Some(first_index) = units.iter().position(|unit| unit.value > 0) else {
        return format!("{}{}", format_relative_age_second_number(0), "s");
    };
    let first = units[first_index];
    if first.suffix == "s" {
        return format!(
            "{}{}",
            format_relative_age_second_number(first.value),
            first.suffix
        );
    }

    let second = units
        .iter()
        .skip(first_index + 1)
        .find(|unit| unit.value > 0)
        .copied()
        .unwrap_or_else(|| units[(first_index + 1).min(units.len() - 1)]);
    format!(
        "{}{}·{}{}",
        first.value,
        first.suffix,
        format_relative_age_second_number(second.value),
        second.suffix
    )
}

/// 低位数字固定双列（如 `5` → `05`），用于 `·` 后段与仅 `s` 段，紧接单位。
fn format_relative_age_second_number(value: i64) -> String {
    let digits = value.max(0).to_string();
    let pad = RELATIVE_AGE_SECOND_NUMBER_DISPLAY_WIDTH.saturating_sub(display_width(&digits));
    format!("{}{digits}", "0".repeat(pad))
}

/// 在固定列宽内渲染相对时间；两段式标签将 `·` 对齐到同一显示列。
pub(crate) fn relative_age_label_fixed_column(
    now_ms: i64,
    timestamp_ms: i64,
    column_width: usize,
    before_dot_width: usize,
) -> String {
    let label = relative_age_label(now_ms, timestamp_ms);
    pad_relative_age_label_to_column(&label, column_width, before_dot_width)
}

/// Branch picker 行内时间格（7 显示列、与列表同一标签与对齐策略）。
pub(crate) fn relative_age_label_table_field(now_ms: i64, timestamp_ms: i64) -> String {
    relative_age_label_fixed_column(
        now_ms,
        timestamp_ms,
        RELATIVE_AGE_BRANCH_PICKER_TIME_COLUMN_WIDTH,
        RELATIVE_AGE_BRANCH_PICKER_BEFORE_DOT_WIDTH,
    )
}

/// 在固定显示列宽内左对齐文本（用于与 `{:<n}` 字符宽区分）。
pub(crate) fn pad_display_width_left(text: &str, width: usize) -> String {
    let width = width.max(1);
    let current = display_width(text);
    if current >= width {
        return truncate_display_width_end(text, width);
    }
    format!("{}{text}", " ".repeat(width - current))
}

fn pad_relative_age_label_to_column(
    label: &str,
    column_width: usize,
    before_dot_width: usize,
) -> String {
    let column_width = column_width.max(1);
    let before_dot_width = before_dot_width.min(column_width.saturating_sub(1));

    let mut formatted = if label == "now" || label == "—" {
        let pad = column_width.saturating_sub(display_width(label));
        format!("{}{label}", " ".repeat(pad))
    } else if let Some((before, after)) = label.split_once('·') {
        let pad = before_dot_width.saturating_sub(display_width(before));
        format!("{}{before}·{after}", " ".repeat(pad))
    } else {
        // 仅一段（如 `42s`）：整体对齐到与两段式「低位数字列」相同起点，不靠 `·` 后空格。
        let second_segment_start = before_dot_width.saturating_add(display_width("·"));
        format!("{}{label}", " ".repeat(second_segment_start))
    };

    let current = display_width(&formatted);
    if current < column_width {
        formatted.push_str(&" ".repeat(column_width - current));
    } else if current > column_width {
        formatted = truncate_display_width_end(&formatted, column_width);
    }
    formatted
}

fn truncate_display_width_end(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = crate::display_width::char_display_width(ch);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_age_label_uses_two_highest_units() {
        let now_ms = 1_800_000_000_000;
        assert_eq!(relative_age_label(now_ms, now_ms), "now");
        assert_eq!(relative_age_label(now_ms, now_ms - 42_000), "42s");
        assert_eq!(relative_age_label(now_ms, now_ms - 5_000), "05s");
        assert_eq!(relative_age_label(now_ms, now_ms - 125_000), "2m·05s");
        assert_eq!(relative_age_label(now_ms, now_ms - 7_200_000), "2h·00m");
        assert!(!relative_age_label(now_ms, now_ms - 125_000).contains("· "));
    }

    #[test]
    fn fixed_column_right_aligns_seconds_only_label() {
        let w = RELATIVE_AGE_LIST_COLUMN_WIDTH;
        let d = RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH;
        let seconds_only = pad_relative_age_label_to_column("42s", w, d);
        let two_part = pad_relative_age_label_to_column("2m·05s", w, d);
        assert_eq!(display_width(&seconds_only), w);
        assert_eq!(display_width(&two_part), w);
        let suffix_col = |s: &str| -> usize {
            s.char_indices()
                .rev()
                .find(|(_, ch)| ch.is_ascii_alphabetic())
                .map(|(i, _)| display_width(&s[..=i]))
                .unwrap_or(0)
        };
        assert_eq!(
            suffix_col(&seconds_only),
            suffix_col(&two_part),
            "seconds-only `s` column should match two-part: {seconds_only:?} vs {two_part:?}"
        );
    }

    #[test]
    fn fixed_column_aligns_unit_suffix_after_dot() {
        let w = RELATIVE_AGE_LIST_COLUMN_WIDTH;
        let d = RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH;
        let a = pad_relative_age_label_to_column("2m·05s", w, d);
        let b = pad_relative_age_label_to_column("2m·12s", w, d);
        let suffix_col = |s: &str| -> usize {
            s.char_indices()
                .rev()
                .find(|(_, ch)| ch.is_ascii_alphabetic())
                .map(|(i, _)| display_width(&s[..=i]))
                .unwrap_or(0)
        };
        assert_eq!(suffix_col(&a), suffix_col(&b), "{a:?} vs {b:?}");
        assert!(a.contains("·05s") || a.contains("·12s"));
        assert!(!a.contains("· "));
    }

    #[test]
    fn branch_picker_table_field_uses_seven_display_columns() {
        let now_ms = 1_800_000_000_000;
        let field = relative_age_label_table_field(now_ms, now_ms - 125_000);
        assert_eq!(
            display_width(&field),
            RELATIVE_AGE_BRANCH_PICKER_TIME_COLUMN_WIDTH
        );
        assert!(field.contains("2m·05s") || field.trim().contains("2m·05s"));
        let a = relative_age_label_table_field(now_ms, now_ms - 125_000);
        let b = relative_age_label_table_field(now_ms, now_ms - 12 * 60_000 - 34_000);
        assert_eq!(a.find('·'), b.find('·'), "{a:?} vs {b:?}");
        let suffix_col = |s: &str| -> usize {
            s.char_indices()
                .rev()
                .find(|(_, ch)| ch.is_ascii_alphabetic())
                .map(|(i, _)| display_width(&s[..=i]))
                .unwrap_or(0)
        };
        assert_eq!(suffix_col(&a), suffix_col(&b));
    }

    #[test]
    fn fixed_column_aligns_dot_across_mixed_lengths() {
        let w = RELATIVE_AGE_LIST_COLUMN_WIDTH;
        let d = RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH;
        let short = pad_relative_age_label_to_column("2m·05s", w, d);
        let long = pad_relative_age_label_to_column("12m·34s", w, d);
        assert_eq!(display_width(&short), w);
        assert_eq!(display_width(&long), w);
        assert_eq!(
            short.find('·'),
            long.find('·'),
            "dot should share the same byte index when padded: {short:?} vs {long:?}"
        );
        assert_eq!(
            display_width(&short[..short.find('·').unwrap()]),
            d,
            "before-dot display width: {short:?}"
        );
        assert_eq!(
            display_width(&long[..long.find('·').unwrap()]),
            d,
            "before-dot display width: {long:?}"
        );
    }
}
