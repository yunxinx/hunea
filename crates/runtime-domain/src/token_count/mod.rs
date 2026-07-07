mod encoding;
mod progress;

pub use encoding::{TokenEncoding, estimate_text_tokens};
pub use progress::StreamingTokenProgress;

const APPROX_BYTES_PER_TOKEN: usize = 4;

/// 图片输入在无法从 provider 获得精确 usage 时的统一估算值。
///
/// 该值对齐 pi 的 `ESTIMATED_IMAGE_CHARS = 4800` / 4 token 启发式，
/// 用于 TUI 进度和 context budget 的稳定估算，而不是按 base64 传输体计数。
pub const ESTIMATED_IMAGE_TOKENS: usize = 1_200;

/// `approximate_tokens_from_bytes` 为无法使用 tokenizer 时提供统一的粗略回退。
pub(crate) fn approximate_tokens_from_bytes(bytes: usize) -> usize {
    bytes.saturating_add(APPROX_BYTES_PER_TOKEN - 1) / APPROX_BYTES_PER_TOKEN
}
