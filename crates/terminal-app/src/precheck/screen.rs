//! PrecheckScreen 编排：step 路由 + event loop。

use color_eyre::eyre::{Result, WrapErr};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::widgets::Clear;
use runtime_domain::paths::DataDirResolution;
use terminal_ui::theme::default_palette;

use super::config_probe::write_portable_marker;
use super::step::{KeyboardHandler, PrecheckStep, StepRenderer, StepState, StepStateProvider};
use super::steps::{
    ConfigAccessibilityWidget, ConfirmSelection, PortableModeConfirmWidget,
    PortableModeRecoveryWidget, RecoverySelection,
};
use super::{Accessibility, PortableMarkerProbe, PrecheckContext, PrecheckResult};
use runtime_domain::paths::WORKSPACE_HUNEA_DIRNAME;
use terminal_ui::MinimalTerminalSession;

pub(crate) struct PrecheckScreen {
    steps: Vec<PrecheckStep>,
    working_dir: std::path::PathBuf,
    data_dir_resolution: DataDirResolution,
    should_exit: bool,
}

impl PrecheckScreen {
    /// 根据 PrecheckContext 条件加入 step（参见 design.md §8.3 路由表）。
    pub(crate) fn new(ctx: &PrecheckContext, initial_resolution: DataDirResolution) -> Self {
        let palette = default_palette();
        let steps = plan_steps(ctx, palette);
        Self {
            steps,
            working_dir: ctx.working_dir.clone(),
            data_dir_resolution: initial_resolution,
            should_exit: false,
        }
    }

    /// 是否存在仍需用户交互的 step。无则调用方不应进入 alternate screen。
    pub(crate) fn needs_interaction(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.step_state(), StepState::InProgress))
    }

    /// 运行 event loop，完成后返回 PrecheckResult。
    pub(crate) fn run(mut self, session: &mut MinimalTerminalSession) -> Result<PrecheckResult> {
        while !self.is_done() {
            self.draw(session)?;
            if let Event::Key(key) = event::read().wrap_err("read precheck terminal event")? {
                self.handle_key_event(key);
                self.apply_step_outcomes()?;
            }
        }
        Ok(self.into_result())
    }

    fn is_done(&self) -> bool {
        self.should_exit || !self.needs_interaction()
    }

    fn handle_key_event(&mut self, key: crossterm::event::KeyEvent) {
        // 唯一 Release 过滤点：widget 不再重复判断 key.kind。
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        if let Some(active) = self.current_step_mut() {
            active.handle_key_event(key);
        }
    }

    fn current_step_mut(&mut self) -> Option<&mut PrecheckStep> {
        self.steps
            .iter_mut()
            .find(|s| matches!(s.step_state(), StepState::InProgress))
    }

    /// 从 step 状态同步副作用：写入便携标记、更新 resolution、设置退出标志。
    ///
    /// I/O（写 marker）放在 screen 而非 widget：widget 只表达选择状态，
    /// 这样 Yes 路径在写盘失败时能把错误冒泡给 event loop，而不是卡在 InProgress。
    /// `activated` 标志防止同一 key 轮次重复写 marker。
    fn apply_step_outcomes(&mut self) -> Result<()> {
        let mut should_exit = false;
        let mut new_resolution: Option<DataDirResolution> = None;

        for step in &mut self.steps {
            match step {
                PrecheckStep::PortableModeConfirm(w) => match w.selection {
                    Some(ConfirmSelection::No) => {
                        should_exit = true;
                    }
                    // Yes 且尚未落盘：写 portable.marker，并把 data dir 切到工作区。
                    Some(ConfirmSelection::Yes) if !w.activated => {
                        write_portable_marker(&w.working_dir).wrap_err("activate portable mode")?;
                        w.activated = true;
                        new_resolution = Some(DataDirResolution::Portable(
                            w.working_dir.join(WORKSPACE_HUNEA_DIRNAME),
                        ));
                    }
                    _ => {}
                },
                PrecheckStep::PortableModeRecovery(w) => {
                    if w.selection == Some(RecoverySelection::Quit) {
                        should_exit = true;
                    }
                }
                PrecheckStep::ConfigAccessibility(_) => {}
            }
        }

        if should_exit {
            self.should_exit = true;
        }
        if let Some(resolution) = new_resolution {
            self.data_dir_resolution = resolution;
        }
        Ok(())
    }

    fn draw(&self, session: &mut MinimalTerminalSession) -> Result<()> {
        session
            .terminal_mut()
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Clear, area);
                let buf = frame.buffer_mut();

                // 只渲染当前 InProgress 的 step，占满全部区域。
                // Complete 的 step 用户已交互过，不再展示，避免小终端挤掉当前选项。
                if let Some(active) = self.current_step() {
                    active.render(area, buf);
                }
            })
            .wrap_err("draw precheck screen")?;
        Ok(())
    }

    fn current_step(&self) -> Option<&PrecheckStep> {
        self.steps
            .iter()
            .find(|s| matches!(s.step_state(), StepState::InProgress))
    }

    pub(crate) fn into_result(self) -> PrecheckResult {
        PrecheckResult {
            data_dir_resolution: self.data_dir_resolution,
            working_dir: Some(self.working_dir),
            should_exit: self.should_exit,
        }
    }
}

/// 根据 context 决定加入哪些 step（design.md §8.3 路由表）。
fn plan_steps(
    ctx: &PrecheckContext,
    palette: terminal_ui::theme::TerminalPalette,
) -> Vec<PrecheckStep> {
    match (&ctx.portable_marker, &ctx.global_accessibility) {
        (PortableMarkerProbe::WorkspaceInaccessible, _) => Vec::new(),
        (PortableMarkerProbe::Absent, Accessibility::Available) => Vec::new(),
        (PortableMarkerProbe::Absent, Accessibility::Unavailable { .. }) => vec![
            PrecheckStep::ConfigAccessibility(ConfigAccessibilityWidget::new(
                ctx.global_accessibility.clone(),
                palette,
            )),
            PrecheckStep::PortableModeConfirm(PortableModeConfirmWidget::new(
                ctx.working_dir.clone(),
                palette,
            )),
        ],
        (PortableMarkerProbe::Present, Accessibility::Available) => {
            vec![PrecheckStep::PortableModeRecovery(
                PortableModeRecoveryWidget::new(palette),
            )]
        }
        (PortableMarkerProbe::Present, Accessibility::Unavailable { .. }) => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
