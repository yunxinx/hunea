//! 启动 banner 的一次性进入动画状态。

use std::time::{Duration, Instant};

use ratatui::{buffer::Buffer, layout::Rect, style::Color};
use tachyonfx::{Effect, Interpolation, Motion, fx};

/// 启动 banner 动画期间的重绘节奏。
pub(crate) const STARTUP_BANNER_ENTRANCE_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// `StartupBannerEntranceState` 只控制进程启动后的第一次 banner 动画。
#[derive(Debug, Clone)]
pub(crate) struct StartupBannerEntranceState {
    phase: StartupBannerEntrancePhase,
    effect: Option<Effect>,
    last_frame_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupBannerEntrancePhase {
    Pending,
    Running,
    Completed,
}

impl Default for StartupBannerEntranceState {
    fn default() -> Self {
        Self {
            phase: StartupBannerEntrancePhase::Pending,
            effect: None,
            last_frame_at: None,
        }
    }
}

impl StartupBannerEntranceState {
    pub(crate) fn frame_interval_at(&self, _now: Instant) -> Option<Duration> {
        matches!(self.phase, StartupBannerEntrancePhase::Running)
            .then_some(STARTUP_BANNER_ENTRANCE_FRAME_INTERVAL)
    }

    pub(crate) fn apply_at(
        &mut self,
        now: Instant,
        buffer: &mut Buffer,
        area: Rect,
        slide_fill_color: Color,
    ) {
        if matches!(self.phase, StartupBannerEntrancePhase::Completed) {
            return;
        }

        if area.is_empty() {
            self.complete();
            return;
        }

        if matches!(self.phase, StartupBannerEntrancePhase::Pending) {
            self.effect = Some(startup_banner_entrance_effect(slide_fill_color));
            self.last_frame_at = Some(now);
            self.phase = StartupBannerEntrancePhase::Running;
        }

        let elapsed = self
            .last_frame_at
            .map(|last_frame_at| now.saturating_duration_since(last_frame_at))
            .unwrap_or_default();
        self.last_frame_at = Some(now);

        let Some(effect) = &mut self.effect else {
            self.complete();
            return;
        };
        effect.process(elapsed, buffer, area);
        if !effect.running() {
            self.complete();
        }
    }

    pub(crate) fn complete(&mut self) {
        self.phase = StartupBannerEntrancePhase::Completed;
        self.effect = None;
        self.last_frame_at = None;
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(&mut self, now: Instant) {
        if matches!(self.phase, StartupBannerEntrancePhase::Pending) {
            self.effect = Some(startup_banner_entrance_effect(Color::from_u32(0x1d2021)));
            self.last_frame_at = Some(now);
            self.phase = StartupBannerEntrancePhase::Running;
        }
    }

    #[cfg(test)]
    pub(crate) const fn is_completed(&self) -> bool {
        matches!(self.phase, StartupBannerEntrancePhase::Completed)
    }
}

fn startup_banner_entrance_effect(slide_fill_color: Color) -> Effect {
    let slide_timer = (1000, Interpolation::Linear);

    fx::slide_in(Motion::LeftToRight, 10, 0, slide_fill_color, slide_timer)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::startup_banner_entrance_effect;

    #[test]
    fn entrance_uses_only_background_slide_in_effect() {
        let effect = startup_banner_entrance_effect(Color::from_u32(0x1d2021));

        assert_eq!(effect.name(), "slide_in");
    }
}
