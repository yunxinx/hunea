use std::time::{Duration, Instant};

use super::{TokenEncoding, approximate_tokens_from_bytes};

const TOKEN_SNAPSHOT_INTERVAL: Duration = Duration::from_millis(120);
const TOKEN_SNAPSHOT_DELTA_THRESHOLD: usize = 12;

/// `StreamingTokenProgress` 对流式输出做节流 token 估算。
///
/// 这里的计数只用于 TUI 反馈，不作为计费或上下文裁剪依据。
#[derive(Debug, Clone)]
pub struct StreamingTokenProgress {
    encoding: TokenEncoding,
    pending_text: String,
    total_tokens: usize,
    last_snapshot_at: Instant,
    has_snapshot: bool,
}

impl StreamingTokenProgress {
    pub fn new(model_id: impl Into<String>) -> Self {
        Self {
            encoding: TokenEncoding::for_model(&model_id.into()),
            pending_text: String::new(),
            total_tokens: 0,
            last_snapshot_at: Instant::now(),
            has_snapshot: false,
        }
    }

    pub fn observe_delta(&mut self, delta: &str, now: Instant) -> Option<usize> {
        if delta.is_empty() {
            return None;
        }

        self.pending_text.push_str(delta);
        if !self.has_snapshot {
            return self.flush(now);
        }
        if now.saturating_duration_since(self.last_snapshot_at) < TOKEN_SNAPSHOT_INTERVAL {
            if approximate_tokens_from_bytes(self.pending_text.len())
                >= TOKEN_SNAPSHOT_DELTA_THRESHOLD
            {
                return self.flush(now);
            }
            return None;
        }

        self.flush(now)
    }

    pub fn flush(&mut self, now: Instant) -> Option<usize> {
        if self.pending_text.is_empty() {
            return None;
        }

        self.total_tokens = self
            .total_tokens
            .saturating_add(self.encoding.estimate_text(&self.pending_text));
        self.pending_text.clear();
        self.last_snapshot_at = now;
        self.has_snapshot = true;

        Some(self.total_tokens)
    }

    pub fn observe_token_count(&mut self, tokens: usize, now: Instant) -> Option<usize> {
        if tokens == 0 {
            return None;
        }

        self.total_tokens = self.total_tokens.saturating_add(tokens);
        self.last_snapshot_at = now;
        self.has_snapshot = true;

        Some(self.total_tokens)
    }

    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_token_progress_throttles_snapshots_and_flushes_pending_text() {
        let started_at = Instant::now();
        let mut progress = StreamingTokenProgress::new("gpt-4o");

        let first = progress
            .observe_delta("Hello", started_at)
            .expect("first output delta should immediately update the token target");
        assert!(first > 0);
        assert_eq!(
            progress.observe_delta(" world", started_at + Duration::from_millis(50)),
            None,
            "token snapshots should be throttled between checks"
        );

        let second = progress
            .observe_delta(" from hunea", started_at + Duration::from_millis(120))
            .expect("throttled snapshot should be emitted");
        assert!(second > first);

        assert_eq!(
            progress.observe_delta("!", started_at + Duration::from_millis(130)),
            None
        );
        let final_total = progress
            .flush(started_at + Duration::from_millis(140))
            .expect("finish should flush pending text");

        assert!(final_total > second);
    }

    #[test]
    fn streaming_token_progress_flushes_large_delta_before_time_interval() {
        let started_at = Instant::now();
        let mut progress = StreamingTokenProgress::new("gpt-4o");

        let first = progress
            .observe_delta("hello", started_at)
            .expect("first delta should be visible immediately");
        let second = progress
            .observe_delta(
                " token token token token token token token token token token token token",
                started_at + Duration::from_millis(20),
            )
            .expect("large pending token delta should not wait for the time interval");

        assert!(second > first);
    }

    #[test]
    fn streaming_token_progress_observes_precomputed_token_count() {
        let started_at = Instant::now();
        let mut progress = StreamingTokenProgress::new("gpt-4o");

        assert_eq!(progress.observe_token_count(0, started_at), None);
        assert_eq!(progress.observe_token_count(12, started_at), Some(12));
        assert_eq!(
            progress.observe_token_count(8, started_at + Duration::from_millis(1)),
            Some(20)
        );
        assert_eq!(progress.total_tokens(), 20);
    }
}
