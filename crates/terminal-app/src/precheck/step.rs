//! Step 抽象：每个预检步骤实现状态查询、键盘处理与渲染。
//!
//! 不设 `Hidden` 变体：条件不满足的 step 在 `plan_steps` 阶段就不加入 `Vec`，
//! 比“加入后再标 Hidden 跳过”更简单，也避免 event loop 多一层过滤。

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use super::steps;

/// 单个 step 的完成状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepState {
    /// 展示并等待用户交互
    InProgress,
    /// 已完成，推进到下一个 InProgress step
    Complete,
}

pub(crate) trait StepStateProvider {
    fn step_state(&self) -> StepState;
}

pub(crate) trait KeyboardHandler {
    fn handle_key_event(&mut self, key: KeyEvent);
}

pub(crate) trait StepRenderer {
    fn render(&self, area: Rect, buf: &mut Buffer);
}

/// 预检流程持有的 step 集合。新增 step 只需加变体 + 实现 trait。
pub(crate) enum PrecheckStep {
    ConfigAccessibility(steps::ConfigAccessibilityWidget),
    PortableModeConfirm(steps::PortableModeConfirmWidget),
    PortableModeRecovery(steps::PortableModeRecoveryWidget),
    SearchToolPrecheck(steps::SearchToolPrecheckWidget),
}

impl StepStateProvider for PrecheckStep {
    fn step_state(&self) -> StepState {
        match self {
            Self::ConfigAccessibility(w) => w.step_state(),
            Self::PortableModeConfirm(w) => w.step_state(),
            Self::PortableModeRecovery(w) => w.step_state(),
            Self::SearchToolPrecheck(w) => w.step_state(),
        }
    }
}

impl KeyboardHandler for PrecheckStep {
    fn handle_key_event(&mut self, key: KeyEvent) {
        match self {
            Self::ConfigAccessibility(w) => w.handle_key_event(key),
            Self::PortableModeConfirm(w) => w.handle_key_event(key),
            Self::PortableModeRecovery(w) => w.handle_key_event(key),
            Self::SearchToolPrecheck(w) => w.handle_key_event(key),
        }
    }
}

impl StepRenderer for PrecheckStep {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        match self {
            Self::ConfigAccessibility(w) => w.render(area, buf),
            Self::PortableModeConfirm(w) => w.render(area, buf),
            Self::PortableModeRecovery(w) => w.render(area, buf),
            Self::SearchToolPrecheck(w) => w.render(area, buf),
        }
    }
}
