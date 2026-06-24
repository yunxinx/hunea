//! 墙钟 Unix 毫秒时间戳；持久化与 TUI 展示共用，避免各层各自解析 `SystemTime`。

use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

/// 无法得到合法的 Unix 毫秒时间戳（时钟早于 epoch 或超出 `i64` 毫秒范围）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnixTimestampError;

impl fmt::Display for UnixTimestampError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "system clock is before Unix epoch or exceeds i64 millisecond range"
        )
    }
}

impl std::error::Error for UnixTimestampError {}

/// 当前墙钟时间的 Unix 毫秒时间戳。
pub fn unix_timestamp_ms() -> Result<i64, UnixTimestampError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| UnixTimestampError)?;
    i64::try_from(duration.as_millis()).map_err(|_| UnixTimestampError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_timestamp_ms_is_positive_on_sane_clock() {
        let ts = unix_timestamp_ms().expect("test environment has sane clock");
        assert!(ts > 0);
    }
}
