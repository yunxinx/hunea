use std::time::Duration;

/// `RuntimeRequestPolicy` 描述交互式 runtime 请求的超时与重试策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequestPolicy {
    attempts: usize,
    delays: Vec<Duration>,
    timeout: Duration,
}

impl RuntimeRequestPolicy {
    /// `new` 使用秒级配置创建 runtime 请求策略。
    pub fn new(attempts: usize, delays_seconds: Vec<u64>, timeout_seconds: u64) -> Self {
        Self {
            attempts,
            delays: delays_seconds
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            timeout: Duration::from_secs(timeout_seconds),
        }
    }

    pub(crate) fn attempts(&self) -> usize {
        self.attempts
    }

    pub(crate) fn delay_for_retry(&self, retry: usize) -> Duration {
        self.delays
            .get(retry.saturating_sub(1))
            .copied()
            .or_else(|| self.delays.last().copied())
            .unwrap_or_else(|| Duration::from_secs(1))
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for RuntimeRequestPolicy {
    fn default() -> Self {
        Self::new(3, vec![1, 2, 3], 120)
    }
}
