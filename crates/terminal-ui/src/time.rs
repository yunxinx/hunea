use runtime_domain::time::unix_timestamp_ms;

pub(crate) fn current_unix_timestamp_ms() -> i64 {
    unix_timestamp_ms().unwrap_or(0)
}
