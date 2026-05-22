use std::time::Duration;

/// `RuntimeRequestMetrics` 记录一次成功 runtime 请求的交互性能指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeRequestMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

impl RuntimeRequestMetrics {
    /// `new` 创建一次 runtime 请求指标。
    pub const fn new(latency: Duration, output_tokens: usize, duration: Duration) -> Self {
        Self {
            latency,
            output_tokens,
            duration,
        }
    }
}
