//! PrecheckScreen 编排：step 路由 + event loop。

use std::time::Duration;

use color_eyre::eyre::{Result, WrapErr};
use crossterm::event::{self, Event, KeyEventKind};
use ratatui::widgets::Clear;
use runtime_domain::paths::DataDirResolution;
use terminal_ui::theme::default_palette;
use tool_runtime::builtin::{ManagedToolKind, ManagedToolStatus, detect_managed_tool_status};

use super::config_probe::write_portable_marker;
use super::managed_search::{ManagedSearchOutcome, persist_managed_search_outcome};
use super::step::{KeyboardHandler, PrecheckStep, StepRenderer, StepState, StepStateProvider};
use super::steps::{
    ConfigAccessibilityWidget, ConfirmSelection, PortableModeConfirmWidget,
    PortableModeRecoveryWidget, RecoverySelection, SearchToolPrecheckWidget,
};
use super::{Accessibility, PortableMarkerProbe, PrecheckContext, PrecheckResult};
use runtime_domain::paths::{CONFIG_FILE_NAME, WORKSPACE_HUNEA_DIRNAME};
use terminal_ui::MinimalTerminalSession;

pub(crate) struct PrecheckScreen {
    steps: Vec<PrecheckStep>,
    working_dir: std::path::PathBuf,
    data_dir_resolution: DataDirResolution,
    should_exit: bool,
    managed_search_outcomes: Vec<ManagedSearchOutcome>,
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
            managed_search_outcomes: Vec::new(),
        }
    }

    /// 是否存在仍需用户交互的 step。无则调用方不应进入 alternate screen。
    pub(crate) fn needs_interaction(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.step_state(), StepState::InProgress))
    }

    /// 有下载时 poll(50ms) 捞进度；无下载时阻塞 read，避免空转重绘。
    pub(crate) fn run(mut self, session: &mut MinimalTerminalSession) -> Result<PrecheckResult> {
        while !self.is_done() {
            self.draw(session)?;
            if self.has_active_download() {
                if event::poll(Duration::from_millis(50))
                    .wrap_err("poll precheck terminal event")?
                    && let Event::Key(key) =
                        event::read().wrap_err("read precheck terminal event")?
                {
                    self.handle_key_event(key);
                }
                self.poll_download_progress();
            } else if let Event::Key(key) =
                event::read().wrap_err("read precheck terminal event")?
            {
                self.handle_key_event(key);
            }
            self.apply_step_outcomes()?;
        }
        Ok(self.into_result())
    }

    fn is_done(&self) -> bool {
        self.should_exit || !self.needs_interaction()
    }

    fn has_active_download(&self) -> bool {
        self.steps.iter().any(|step| {
            if let PrecheckStep::SearchToolPrecheck(w) = step {
                w.is_downloading()
            } else {
                false
            }
        })
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

    fn poll_download_progress(&mut self) {
        if let Some(PrecheckStep::SearchToolPrecheck(w)) = self.current_step_mut() {
            w.poll_progress();
        }
    }

    /// 同步 step 副作用。写 marker 放 screen 而非 widget，便于 I/O 错误冒泡。
    fn apply_step_outcomes(&mut self) -> Result<()> {
        let mut should_exit = false;
        let mut new_resolution: Option<DataDirResolution> = None;
        let mut new_outcomes: Vec<ManagedSearchOutcome> = Vec::new();

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
                PrecheckStep::SearchToolPrecheck(w) => {
                    if w.step_state() == StepState::Complete {
                        if let Some(outcome) = w.take_outcome() {
                            new_outcomes.push(outcome);
                        }
                        if w.wants_exit() {
                            should_exit = true;
                        }
                    }
                }
            }
        }

        if should_exit {
            self.should_exit = true;
        }
        if let Some(resolution) = new_resolution {
            let new_managed_root = resolution.config_dir().to_path_buf();
            self.data_dir_resolution = resolution;
            // 便携切换后更新 widget root；进行中的下载仍用旧 root。
            for step in &mut self.steps {
                if let PrecheckStep::SearchToolPrecheck(w) = step {
                    w.set_managed_root(new_managed_root.clone());
                }
            }
        }
        // write-through：避免后续 step Quit 丢掉已完成工具的授权。
        if !new_outcomes.is_empty() {
            let config_path = self.data_dir_resolution.config_dir().join(CONFIG_FILE_NAME);
            for outcome in &new_outcomes {
                persist_managed_search_outcome(outcome, &config_path);
            }
            self.managed_search_outcomes.extend(new_outcomes);
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

    pub(crate) fn into_result(mut self) -> PrecheckResult {
        // 有 Drop，字段用 take/clone 取出。
        PrecheckResult {
            data_dir_resolution: self.data_dir_resolution.clone(),
            working_dir: Some(std::mem::take(&mut self.working_dir)),
            should_exit: self.should_exit,
            managed_search_outcomes: std::mem::take(&mut self.managed_search_outcomes),
        }
    }
}

impl Drop for PrecheckScreen {
    fn drop(&mut self) {
        for step in &mut self.steps {
            if let PrecheckStep::SearchToolPrecheck(w) = step {
                w.abort_download();
            }
        }
    }
}

fn plan_steps(
    ctx: &PrecheckContext,
    palette: terminal_ui::theme::TerminalPalette,
) -> Vec<PrecheckStep> {
    let mut steps = Vec::new();

    match (&ctx.portable_marker, &ctx.global_accessibility) {
        (PortableMarkerProbe::WorkspaceInaccessible, _) => {}
        (PortableMarkerProbe::Absent, Accessibility::Available) => {}
        (PortableMarkerProbe::Absent, Accessibility::Unavailable { .. }) => {
            steps.push(PrecheckStep::ConfigAccessibility(
                ConfigAccessibilityWidget::new(ctx.global_accessibility.clone(), palette),
            ));
            steps.push(PrecheckStep::PortableModeConfirm(
                PortableModeConfirmWidget::new(ctx.working_dir.clone(), palette),
            ));
        }
        (PortableMarkerProbe::Present, Accessibility::Available) => {
            steps.push(PrecheckStep::PortableModeRecovery(
                PortableModeRecoveryWidget::new(palette),
            ));
        }
        (PortableMarkerProbe::Present, Accessibility::Unavailable { .. }) => {}
    }

    // rg 先、fd 后；就绪/已拒绝不加 step。
    for tool in [ManagedToolKind::Ripgrep, ManagedToolKind::Fd] {
        let status =
            detect_managed_tool_status(tool, &ctx.managed_search_config, &ctx.managed_root);
        match status {
            ManagedToolStatus::SystemPath(_)
            | ManagedToolStatus::Bundled(_)
            | ManagedToolStatus::ManagedReady(_)
            | ManagedToolStatus::NotAuthorized => {}
            ManagedToolStatus::NeedsDownload => {
                steps.push(PrecheckStep::SearchToolPrecheck(
                    SearchToolPrecheckWidget::new(
                        tool,
                        false,
                        palette,
                        ctx.managed_root.clone(),
                        false,
                    ),
                ));
            }
            ManagedToolStatus::NeedsRebuild => {
                steps.push(PrecheckStep::SearchToolPrecheck(
                    SearchToolPrecheckWidget::new(
                        tool,
                        false,
                        palette,
                        ctx.managed_root.clone(),
                        true,
                    ),
                ));
            }
            ManagedToolStatus::AndroidIncompatible => {
                steps.push(PrecheckStep::SearchToolPrecheck(
                    SearchToolPrecheckWidget::new(
                        tool,
                        true,
                        palette,
                        ctx.managed_root.clone(),
                        false,
                    ),
                ));
            }
        }
    }

    steps
}

#[cfg(test)]
mod tests;
