mod encoding;
mod progress;

pub use encoding::{TokenEncoding, estimate_text_tokens};
pub use progress::StreamingTokenProgress;

const APPROX_BYTES_PER_TOKEN: usize = 4;

/// `approximate_tokens_from_bytes` 为无法使用 tokenizer 时提供统一的粗略回退。
pub(crate) fn approximate_tokens_from_bytes(bytes: usize) -> usize {
    bytes.saturating_add(APPROX_BYTES_PER_TOKEN - 1) / APPROX_BYTES_PER_TOKEN
}
