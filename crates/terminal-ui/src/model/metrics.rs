use std::time::Duration;

use crate::transcript::TranscriptEstimateBreakdown;

/// `RequestMetrics` 保存最近一次成功完成请求的 LLM 输出状态行指标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestMetrics {
    pub latency: Duration,
    pub output_tokens: usize,
    pub duration: Duration,
}

impl RequestMetrics {
    /// `new` 创建最近一次成功请求的 LLM 输出性能指标。
    pub fn new(latency: Duration, output_tokens: usize, duration: Duration) -> Self {
        Self {
            latency,
            output_tokens,
            duration,
        }
    }
}

/// `TranscriptSyncProfile` 记录一次 transcript sync 在首帧前的关键耗时拆分。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TranscriptSyncProfile {
    pub(crate) estimate_time: Duration,
    pub(crate) visible_exact_time: Duration,
    pub(crate) estimate_breakdown: TranscriptEstimateBreakdown,
}
