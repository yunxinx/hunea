//! 列表行内相对时间标签（与 entry tree branch picker 一致的两档单位格式）。

use crate::display_width::display_width;

/// 固定显示列宽内对齐 `·`，用于全屏列表时间列。
pub(crate) const RELATIVE_AGE_LIST_COLUMN_WIDTH: usize = 9;
/// `·` 前可见文本占用的显示列宽（含左侧填充），使各行 `·` 落在同一列。
pub(crate) const RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH: usize = 4;

/// 将 `timestamp_ms` 相对 `now_ms` 格式化为紧凑英文标签，例如 `2h·5m`、`3d·2h`。
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
        return "0s".to_string();
    };
    let first = units[first_index];
    if first.suffix == "s" {
        return format!("{}{}", first.value, first.suffix);
    }

    let second = units
        .iter()
        .skip(first_index + 1)
        .find(|unit| unit.value > 0)
        .copied()
        .unwrap_or_else(|| units[(first_index + 1).min(units.len() - 1)]);
    format!(
        "{}{}·{}{}",
        first.value, first.suffix, second.value, second.suffix
    )
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

fn pad_relative_age_label_to_column(
    label: &str,
    column_width: usize,
    before_dot_width: usize,
) -> String {
    let column_width = column_width.max(1);
    let before_dot_width = before_dot_width.min(column_width.saturating_sub(1));

    let mut formatted = if let Some((before, after)) = label.split_once('·') {
        let pad = before_dot_width.saturating_sub(display_width(before));
        format!("{}{before}·{after}", " ".repeat(pad))
    } else {
        label.to_string()
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
        assert_eq!(relative_age_label(now_ms, now_ms - 125_000), "2m·5s");
        assert_eq!(relative_age_label(now_ms, now_ms - 7_200_000), "2h·0m");
    }

    #[test]
    fn fixed_column_aligns_dot_across_mixed_lengths() {
        let w = RELATIVE_AGE_LIST_COLUMN_WIDTH;
        let d = RELATIVE_AGE_LIST_BEFORE_DOT_WIDTH;
        let short = pad_relative_age_label_to_column("2m·5s", w, d);
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
