//! 滚轮平滑滚动状态：事件侧累加增量，渲染帧侧按恒速曲线分次消耗（drain）。
//!
//! 纯状态逻辑：不取 now、不触碰 layout。`Instant` 与档位调参（tuning）一律由
//! 调用方传入（与 `advance_toast_at` 同模式），使全部行为可用固定时刻做单元测试，
//! 且配置档位不进入可被会话重置路径清空的状态结构。

use std::time::{Duration, Instant};

/// drain 动画帧间隔。drain 帧本身是微秒级成本（缓存兜底），8ms 让可见中间帧
/// 数量比 16ms 翻倍，恒速滑动的观感更连续。
const SMOOTH_SCROLL_FRAME_INTERVAL: Duration = Duration::from_millis(8);
/// 相邻滚轮事件间隔在该窗口内视为连续快速滚动，步长倍率爬升。
const WHEEL_ACCEL_WINDOW: Duration = Duration::from_millis(120);
/// 连续滚动时每个事件的倍率增量。
const WHEEL_ACCEL_STEP: f32 = 0.3;
/// 空闲（无加速）时的基线倍率。
const WHEEL_ACCEL_BASE: f32 = 1.0;

/// `ScrollAnimationMode` 表示滚轮平滑滚动的语义档位。
///
/// 语义化档位替代裸参数暴露：不同终端/鼠标驱动组合产生的滚轮事件流特征差异
/// 很大（高频离散 vs 系统预平滑），单一默认曲线无法覆盖，用户按观感选档。
/// `Off` 精确还原瞬时现状（固定步长、无加速度）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollAnimationMode {
    Off,
    Snappy,
    Fast,
    #[default]
    Smooth,
    Gentle,
    Glide,
}

/// `SmoothScrollTuning` 是每个动画档位的调参集。
///
/// 四个字段沿档位表单调有序（instant 阈值递减、行/帧递减、backlog 上限递增、
/// 加速度封顶递增）：档位越靠后动画越长、越"滑"。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SmoothScrollTuning {
    /// pending 不超过该值时一帧直达（慢滚点按期待即时反馈）。
    pub(crate) instant_threshold: usize,
    /// 匀速阶段每帧消耗的行数（恒速才有「滑动」观感；比例式曲线会把大部分
    /// 位移前置到首帧，在 8–16ms 帧率下动画不可感知，已废弃）。
    pub(crate) lines_per_frame: usize,
    /// 参与动画的 backlog 上限；超出部分并入首帧立即消耗（狂滚防追不上）。
    pub(crate) animated_backlog_cap: usize,
    /// 滚轮加速度倍率封顶。
    pub(crate) wheel_accel_max: f32,
}

/// 档位表（design 6.2）：参数逐列单调，便于横向比对与增删档位。
const SNAPPY_TUNING: SmoothScrollTuning = SmoothScrollTuning {
    instant_threshold: 12,
    lines_per_frame: 9,
    animated_backlog_cap: 30,
    wheel_accel_max: 2.0,
};
const FAST_TUNING: SmoothScrollTuning = SmoothScrollTuning {
    instant_threshold: 6,
    lines_per_frame: 6,
    animated_backlog_cap: 45,
    wheel_accel_max: 2.5,
};
const SMOOTH_TUNING: SmoothScrollTuning = SmoothScrollTuning {
    instant_threshold: 3,
    lines_per_frame: 3,
    animated_backlog_cap: 60,
    wheel_accel_max: 3.0,
};
const GENTLE_TUNING: SmoothScrollTuning = SmoothScrollTuning {
    instant_threshold: 2,
    lines_per_frame: 2,
    animated_backlog_cap: 80,
    wheel_accel_max: 3.5,
};
const GLIDE_TUNING: SmoothScrollTuning = SmoothScrollTuning {
    instant_threshold: 1,
    lines_per_frame: 1,
    animated_backlog_cap: 100,
    wheel_accel_max: 4.0,
};

impl ScrollAnimationMode {
    /// 当前档位的 drain 调参；`Off` 没有动画曲线，返回 `None`，
    /// 调用方以此为「平滑路径 vs 瞬时路径」的类型化分派依据。
    pub(crate) const fn tuning(self) -> Option<&'static SmoothScrollTuning> {
        match self {
            Self::Off => None,
            Self::Snappy => Some(&SNAPPY_TUNING),
            Self::Fast => Some(&FAST_TUNING),
            Self::Smooth => Some(&SMOOTH_TUNING),
            Self::Gentle => Some(&GENTLE_TUNING),
            Self::Glide => Some(&GLIDE_TUNING),
        }
    }
}

/// 滚轮平滑滚动状态：pending 累加器 + 加速度倍率 + drain 帧时刻。
///
/// 事件侧调用 [`accumulate_wheel`](Self::accumulate_wheel)，渲染帧侧调用
/// [`drain_step`](Self::drain_step)；边界触达 / 吸附 / 贴底恢复时调用
/// [`clear_pending`](Self::clear_pending)。
#[derive(Debug, Clone)]
pub(crate) struct SmoothScrollState {
    /// 未消耗的滚动行数；正值向下。反向滚动在累加器中自然抵消。
    pending_lines: isize,
    /// 上次滚轮事件时刻，用于加速度窗口判定。
    last_wheel_at: Option<Instant>,
    /// 上次滚轮事件的方向（`signum`）；方向反转视为新手势，倍率重置。
    last_wheel_direction: isize,
    /// 当前步长倍率；窗口内同向爬升、空闲或反向重置为基线。
    wheel_multiplier: f32,
    /// 上一 drain 帧时刻，作为下一动画 deadline 的锚点；drain 收敛后清空。
    last_drain_at: Option<Instant>,
}

impl Default for SmoothScrollState {
    fn default() -> Self {
        Self {
            pending_lines: 0,
            last_wheel_at: None,
            last_wheel_direction: 0,
            // 倍率基线是 1.0 而非数值零值，故手写 Default。
            wheel_multiplier: WHEEL_ACCEL_BASE,
            last_drain_at: None,
        }
    }
}

impl SmoothScrollState {
    /// 按加速度倍率放大 `delta_lines` 后累加进 pending，返回放大后的增量。
    ///
    /// 加速度窗口（120ms）与步进（0.3）是全局常量；封顶来自档位 tuning，
    /// 越"滑"的档位允许更高的封顶来补偿更慢的匀速消耗。
    ///
    /// 倍率只在「窗口内 **且** 同方向」连滚时爬升：空闲超窗或方向反转都视为
    /// 新手势、重置回基线，避免爬到封顶后反向轻滚一格被放大数倍（过冲）。
    ///
    /// 放大结果四舍五入（half away from zero）：正负方向对称，且基线倍率下
    /// 不改变原始行数。
    pub(crate) fn accumulate_wheel(
        &mut self,
        delta_lines: isize,
        now: Instant,
        tuning: &SmoothScrollTuning,
    ) -> isize {
        let direction = delta_lines.signum();
        let within_accel_window = self
            .last_wheel_at
            .is_some_and(|last| now.saturating_duration_since(last) <= WHEEL_ACCEL_WINDOW);
        let same_direction = direction == self.last_wheel_direction;
        self.wheel_multiplier = if within_accel_window && same_direction {
            (self.wheel_multiplier + WHEEL_ACCEL_STEP).min(tuning.wheel_accel_max)
        } else {
            WHEEL_ACCEL_BASE
        };
        self.last_wheel_at = Some(now);
        self.last_wheel_direction = direction;

        let amplified = (delta_lines as f32 * self.wheel_multiplier).round() as isize;
        self.pending_lines = self.pending_lines.saturating_add(amplified);
        amplified
    }

    /// 本帧是否应推进一步 drain：drain 步进按 8ms 时钟节流，而非渲染频率。
    ///
    /// 否则 streaming、连续按键等高频渲染事件会在每帧额外消耗一步，恒速动画
    /// 在 UI 最繁忙时被加速到近似瞬时——与 `next_frame_deadline_at` 锚定的
    /// 最小帧间隔意图自相矛盾。首帧（尚未 drain）立即推进。
    pub(crate) fn ready_to_drain_at(&self, now: Instant) -> bool {
        match self.last_drain_at {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= SMOOTH_SCROLL_FRAME_INTERVAL,
        }
    }

    /// 取出本帧应消耗的行数并从 pending 扣除。
    ///
    /// 恒速步进曲线（design 6.1）：
    /// - `|pending| <= instant_threshold`：一帧直达，慢滚点按保持即时反馈；
    /// - 超出 `animated_backlog_cap` 的部分并入本帧立即消耗（不产生中间帧），
    ///   剩余按 `lines_per_frame` 匀速步进——恒速才有可感知的滑动。
    ///
    /// 帧节流由调用方经 [`ready_to_drain_at`](Self::ready_to_drain_at) 把关：
    /// 本方法一经调用即消耗一步，`now` 记录为本次 drain 帧时刻并锚定下一
    /// 动画 deadline；收敛（pending 清零）后清除锚点，让下一轮滚动从新的
    /// 事件时刻重新起步。
    pub(crate) fn drain_step(&mut self, now: Instant, tuning: &SmoothScrollTuning) -> isize {
        if self.pending_lines == 0 {
            return 0;
        }

        let magnitude = self.pending_lines.unsigned_abs();
        let step_magnitude = if magnitude <= tuning.instant_threshold {
            magnitude
        } else {
            let overflow = magnitude.saturating_sub(tuning.animated_backlog_cap);
            // 全档位 instant_threshold >= lines_per_frame，尾部必先落入 instant
            // 直达分支；min 只是防御未来档位破坏该关系时的越扣。
            overflow + tuning.lines_per_frame.min(magnitude - overflow)
        };
        let step = self.pending_lines.signum() * step_magnitude as isize;
        self.pending_lines -= step;

        if self.pending_lines == 0 {
            self.last_drain_at = None;
        } else {
            self.last_drain_at = Some(now);
        }
        step
    }

    /// 清空累加器：边界触达 / restore-target 吸附 / 贴底恢复时调用，
    /// 防止边界外积压的 delta 造成反向粘滞。
    ///
    /// 同时把加速度倍率重置回基线：清空标志本次滚动手势的终点，残留的高
    /// 倍率会把下一次（常是反向修正）滚动放大，与「清空防粘滞」的目的相悖。
    pub(crate) fn clear_pending(&mut self) {
        self.pending_lines = 0;
        // deadline 锚点只对进行中的 drain 有意义，一并清除。
        self.last_drain_at = None;
        self.wheel_multiplier = WHEEL_ACCEL_BASE;
        self.last_wheel_direction = 0;
    }

    /// pending 非零，即 drain 动画仍在收敛中。
    pub(crate) fn is_settling(&self) -> bool {
        self.pending_lines != 0
    }

    /// 未消耗的滚动行数（正值向下）。exactize 配套据此判定滚动方向
    /// 与剩余距离，只读不改变状态。
    pub(crate) fn pending_lines(&self) -> isize {
        self.pending_lines
    }

    /// 下一 drain 帧的 deadline：锚定上一 drain 帧时刻；尚未 drain 过
    /// （刚累加）时锚定 `now`。pending 清零后不再产生 deadline。
    pub(crate) fn next_frame_deadline_at(&self, now: Instant) -> Option<Instant> {
        if !self.is_settling() {
            return None;
        }
        Some(self.last_drain_at.unwrap_or(now) + SMOOTH_SCROLL_FRAME_INTERVAL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定基准时刻 + 毫秒偏移构造测试时间线，不依赖真实时钟流逝。
    fn timeline() -> impl Fn(u64) -> Instant {
        let base = Instant::now();
        move |millis| base + Duration::from_millis(millis)
    }

    fn smooth() -> &'static SmoothScrollTuning {
        ScrollAnimationMode::Smooth
            .tuning()
            .expect("smooth tier must define a tuning")
    }

    fn animated_tiers_in_order() -> Vec<&'static SmoothScrollTuning> {
        [
            ScrollAnimationMode::Snappy,
            ScrollAnimationMode::Fast,
            ScrollAnimationMode::Smooth,
            ScrollAnimationMode::Gentle,
            ScrollAnimationMode::Glide,
        ]
        .into_iter()
        .map(|mode| mode.tuning().expect("animated tier must define a tuning"))
        .collect()
    }

    #[test]
    fn off_tier_has_no_tuning_and_default_is_smooth() {
        assert!(ScrollAnimationMode::Off.tuning().is_none());
        assert_eq!(ScrollAnimationMode::default(), ScrollAnimationMode::Smooth);
    }

    #[test]
    fn animated_tiers_are_monotonic_across_all_tuning_fields() {
        for pair in animated_tiers_in_order().windows(2) {
            let (shorter, longer) = (pair[0], pair[1]);
            // 越靠后的档位动画越长：instant 阈值与行/帧递减，
            // backlog 上限与加速度封顶递增。
            assert!(shorter.instant_threshold > longer.instant_threshold);
            assert!(shorter.lines_per_frame > longer.lines_per_frame);
            assert!(shorter.animated_backlog_cap < longer.animated_backlog_cap);
            assert!(shorter.wheel_accel_max < longer.wheel_accel_max);
        }
        // 尾部收敛前提：instant 直达分支必须先于「不足一帧步长」出现。
        for tuning in animated_tiers_in_order() {
            assert!(tuning.instant_threshold >= tuning.lines_per_frame);
            assert!(tuning.animated_backlog_cap >= tuning.lines_per_frame);
        }
    }

    #[test]
    fn accumulate_adds_pending_and_reverse_cancels() {
        let at = timeline();
        let mut state = SmoothScrollState::default();

        assert_eq!(state.accumulate_wheel(3, at(0), smooth()), 3);
        assert!(state.is_settling());

        // 间隔超出加速窗口，倍率回到基线，反向增量与正向精确抵消。
        assert_eq!(state.accumulate_wheel(-3, at(500), smooth()), -3);
        assert!(!state.is_settling());
        assert_eq!(state.drain_step(at(600), smooth()), 0);
    }

    #[test]
    fn drain_reaches_small_pending_in_a_single_instant_frame() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(3, at(0), smooth());

        // |pending| <= instant 阈值（smooth = 3）：一帧直达。
        assert_eq!(state.drain_step(at(8), smooth()), 3);
        assert!(!state.is_settling());
    }

    #[test]
    fn drain_steps_at_constant_rate_within_backlog_cap() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        // 40 行在 backlog 上限（60）内、超过 instant 阈值：全程匀速。
        state.accumulate_wheel(40, at(0), smooth());

        let steps: Vec<isize> = std::iter::from_fn(|| {
            let step = state.drain_step(at(8), smooth());
            (step != 0).then_some(step)
        })
        .collect();

        // 40 = 3 行/帧 × 12 帧 + 尾部 4 行……尾部 4 > instant 3，仍按 3 步进，
        // 最后 1 行落入 instant 直达。
        assert_eq!(steps.len(), 14);
        assert!(steps[..13].iter().all(|step| *step == 3));
        assert_eq!(*steps.last().expect("burst must produce steps"), 1);
        assert_eq!(steps.iter().sum::<isize>(), 40);
    }

    #[test]
    fn drain_treats_pending_exactly_at_backlog_cap_as_fully_animated() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        // 恰等于 backlog 上限（smooth = 60）：溢出为零，首帧不并入超量，
        // 全程按 3 行/帧匀速收敛（60 = 3 × 20 帧，尾帧恰落 instant 直达）。
        state.accumulate_wheel(60, at(0), smooth());

        let steps: Vec<isize> = std::iter::from_fn(|| {
            let step = state.drain_step(at(8), smooth());
            (step != 0).then_some(step)
        })
        .collect();

        assert_eq!(steps.len(), 20);
        assert!(steps.iter().all(|step| *step == 3));
        assert_eq!(steps.iter().sum::<isize>(), 60);
    }

    #[test]
    fn drain_consumes_backlog_overflow_immediately_then_steps_uniformly() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(200, at(0), smooth());

        // 200 超出 backlog 上限 60：溢出 140 并入首帧（140 + 3 行/帧 = 143），
        // 其余 57 行按 3 行/帧匀速收敛，不产生追不上的长尾。
        assert_eq!(state.drain_step(at(8), smooth()), 143);
        let mut uniform_steps = 0;
        while state.is_settling() {
            assert_eq!(state.drain_step(at(16), smooth()), 3);
            uniform_steps += 1;
        }
        assert_eq!(uniform_steps, 19);
    }

    #[test]
    fn drain_is_symmetric_for_upward_scroll() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(-100, at(0), smooth());

        let steps: Vec<isize> = std::iter::from_fn(|| {
            let step = state.drain_step(at(8), smooth());
            (step != 0).then_some(step)
        })
        .collect();

        // 溢出 40 并入首帧（-43），剩余 57 行按 -3/帧匀速，与正向逐帧镜像。
        assert_eq!(steps[0], -43);
        assert!(steps[1..].iter().all(|step| *step == -3));
        assert_eq!(steps.iter().sum::<isize>(), -100);
    }

    #[test]
    fn snappy_tier_finishes_instant_sized_pending_in_one_frame() {
        let at = timeline();
        let tuning = ScrollAnimationMode::Snappy
            .tuning()
            .expect("snappy tier must define a tuning");
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(12, at(0), tuning);

        // snappy 的 instant 阈值 12：普通单格~连滚小增量一帧直达，接近瞬时。
        assert_eq!(state.drain_step(at(8), tuning), 12);
        assert!(!state.is_settling());
    }

    #[test]
    fn wheel_acceleration_ramps_within_window_and_caps_at_tuning_max() {
        let at = timeline();
        let mut state = SmoothScrollState::default();

        // 首个事件无前驱，倍率为基线。
        assert_eq!(state.accumulate_wheel(3, at(0), smooth()), 3);
        // 100ms 间隔在 120ms 窗口内：1.3 → 1.6 逐步爬升。
        assert_eq!(state.accumulate_wheel(3, at(100), smooth()), 4);
        assert_eq!(state.accumulate_wheel(3, at(200), smooth()), 5);

        // 持续连滚直至档位封顶：smooth 3.0 → 3 行/格 × 3.0 = 9。
        let mut last_amplified = 0;
        for tick in 3..12 {
            last_amplified = state.accumulate_wheel(3, at(tick * 100), smooth());
        }
        assert_eq!(last_amplified, 9);
    }

    #[test]
    fn wheel_acceleration_cap_follows_tier_tuning() {
        let at = timeline();
        let snappy = ScrollAnimationMode::Snappy
            .tuning()
            .expect("snappy tier must define a tuning");
        let glide = ScrollAnimationMode::Glide
            .tuning()
            .expect("glide tier must define a tuning");

        // 同样的连滚节奏，封顶随档位不同：snappy 2.0 → 6；glide 4.0 → 12。
        let mut snappy_state = SmoothScrollState::default();
        let mut glide_state = SmoothScrollState::default();
        let (mut snappy_last, mut glide_last) = (0, 0);
        for tick in 0..16 {
            snappy_last = snappy_state.accumulate_wheel(3, at(tick * 100), snappy);
            glide_last = glide_state.accumulate_wheel(3, at(tick * 100), glide);
        }
        assert_eq!(snappy_last, 6);
        assert_eq!(glide_last, 12);
    }

    #[test]
    fn wheel_acceleration_resets_after_idle_gap() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        for tick in 0..12 {
            state.accumulate_wheel(3, at(tick * 100), smooth());
        }

        // 空闲超过加速窗口后回到基线倍率。
        assert_eq!(state.accumulate_wheel(3, at(2000), smooth()), 3);
    }

    #[test]
    fn wheel_acceleration_resets_on_direction_reversal() {
        let at = timeline();
        let mut state = SmoothScrollState::default();

        // 同向连滚爬到较高倍率。
        let mut amplified = 0;
        for tick in 0..6 {
            amplified = state.accumulate_wheel(3, at(tick * 20), smooth());
        }
        assert!(amplified > 3, "同向连滚应放大");

        // 窗口内立即反向：视为新手势，倍率重置回基线，反向单格不被放大。
        assert_eq!(state.accumulate_wheel(-3, at(120), smooth()), -3);
    }

    #[test]
    fn clear_pending_resets_acceleration_to_baseline() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        // 连滚爬升倍率后（如触边界）清空。
        for tick in 0..6 {
            state.accumulate_wheel(3, at(tick * 20), smooth());
        }
        state.clear_pending();

        // 清空后窗口内的下一次滚动从基线起步，不被残留倍率放大。
        assert_eq!(state.accumulate_wheel(-3, at(140), smooth()), -3);
    }

    #[test]
    fn drain_is_throttled_to_frame_interval() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(40, at(0), smooth());

        // 首帧（尚未 drain）立即就绪。
        assert!(state.ready_to_drain_at(at(0)));
        state.drain_step(at(0), smooth());

        // 未满 8ms 帧间隔：不就绪，避免高频渲染加速动画。
        assert!(!state.ready_to_drain_at(at(4)));
        // 达到 8ms：就绪。
        assert!(state.ready_to_drain_at(at(8)));
    }

    #[test]
    fn clear_pending_stops_settling_and_deadline() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        state.accumulate_wheel(100, at(0), smooth());
        state.drain_step(at(8), smooth());

        state.clear_pending();

        assert!(!state.is_settling());
        assert_eq!(state.next_frame_deadline_at(at(16)), None);
        assert_eq!(state.drain_step(at(16), smooth()), 0);
    }

    #[test]
    fn deadline_anchors_on_last_drain_and_clears_on_settle() {
        let at = timeline();
        let mut state = SmoothScrollState::default();
        assert_eq!(state.next_frame_deadline_at(at(0)), None);

        state.accumulate_wheel(100, at(0), smooth());
        // 尚未 drain：锚定传入的 now。
        assert_eq!(
            state.next_frame_deadline_at(at(0)),
            Some(at(0) + SMOOTH_SCROLL_FRAME_INTERVAL)
        );

        state.drain_step(at(8), smooth());
        // drain 进行中：锚定上一 drain 帧时刻，而非查询时刻。
        assert_eq!(
            state.next_frame_deadline_at(at(10)),
            Some(at(8) + SMOOTH_SCROLL_FRAME_INTERVAL)
        );

        // drain 至收敛后不再产生 deadline。
        while state.drain_step(at(16), smooth()) != 0 {}
        assert_eq!(state.next_frame_deadline_at(at(24)), None);
    }
}
