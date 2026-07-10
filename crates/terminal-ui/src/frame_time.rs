use std::time::{Duration, Instant};

/// `FrameRenderContext` 保存一次 render tree 共享的单一时钟采样。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameRenderContext {
    now: Instant,
}

impl FrameRenderContext {
    pub(crate) fn capture() -> Self {
        Self::new(Instant::now())
    }

    pub(crate) const fn new(now: Instant) -> Self {
        Self { now }
    }

    pub(crate) const fn now(self) -> Instant {
        self.now
    }
}

/// 返回以 `origin` 为锚点、严格晚于当前 animation frame 的下一个绝对 deadline。
pub(crate) fn next_animation_frame_deadline(
    origin: Instant,
    now: Instant,
    interval: Duration,
) -> Option<Instant> {
    if interval.is_zero() {
        return Some(now);
    }
    if now < origin {
        return origin.checked_add(interval);
    }

    let interval_nanos = interval.as_nanos();
    let elapsed_nanos = now.saturating_duration_since(origin).as_nanos();
    let remaining_nanos = interval_nanos - elapsed_nanos % interval_nanos;
    let remaining = Duration::from_nanos(u64::try_from(remaining_nanos).ok()?);
    now.checked_add(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_deadline_stays_on_origin_anchored_boundaries() {
        let origin = Instant::now();
        let interval = Duration::from_millis(80);

        assert_eq!(
            next_animation_frame_deadline(origin, origin + Duration::from_millis(70), interval,),
            Some(origin + interval),
        );
        assert_eq!(
            next_animation_frame_deadline(origin, origin + interval, interval),
            Some(origin + interval * 2),
        );
    }
}
