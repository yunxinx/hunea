use std::time::Instant;

use color_eyre::eyre::Result;
use mo_core::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent},
};

use super::{AppEvent, HeroOptions, Model, ModelOptions, STARTUP_PROBE_TIMEOUT, StyleMode, theme};

mod effects;
mod event_pipeline;
mod external_io;
mod input;
mod model_refresh;
mod native_agent;
mod terminal;

use super::runtime::RuntimeEventApply;
use effects::apply_effect_if_needed;
use input::{TerminalInputAction, coalesced_input_actions, read_ready_terminal_events};
use model_refresh::apply_model_provider_refresh_event;
use terminal::{TerminalMouseMode, TerminalSession, apply_mouse_mode, wait_for_terminal_event};

/// `RuntimeCoordinator` 是 TUI runner 与具体 agent runtime 之间的最小边界。
pub trait RuntimeCoordinator {
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        Vec::new()
    }

    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        Vec::new()
    }

    fn has_background_runtime(&self) -> bool {
        false
    }

    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> std::result::Result<RuntimeCommandReceipt, String> {
        Err(match command.target() {
            Some(target) => format!("Runtime is not available: {}", target.display_label()),
            None => "Runtime is not available".to_string(),
        })
    }

    fn persist_selected_model(
        &mut self,
        _selection: &ModelSelection,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    fn refresh_model_provider(
        &mut self,
        _request: ProviderSyncRequest,
    ) -> std::result::Result<(), String> {
        Err("Model refresh runtime is not available".to_string())
    }
}

/// `NoopRuntimeCoordinator` 让纯 TUI 构建可以独立运行到模型更新层。
#[derive(Debug, Default)]
pub struct NoopRuntimeCoordinator;

impl RuntimeCoordinator for NoopRuntimeCoordinator {}

/// `run` 启动交互式 TUI，并在退出后返回最终模型。
pub fn run(hero_options: HeroOptions) -> Result<Model> {
    run_with_options(hero_options, ModelOptions::default())
}

/// `run_with_style_mode` 启动带指定样式模式的交互式 TUI。
pub fn run_with_style_mode(hero_options: HeroOptions, style_mode: StyleMode) -> Result<Model> {
    run_with_options(
        hero_options,
        ModelOptions {
            style_mode,
            ..ModelOptions::default()
        },
    )
}

/// `run_with_options` 启动带显式选项的交互式 TUI。
pub fn run_with_options(hero_options: HeroOptions, options: ModelOptions) -> Result<Model> {
    let mut runtime_coordinator = NoopRuntimeCoordinator;
    run_with_runtime_coordinator(hero_options, options, &mut runtime_coordinator)
}

/// `run_with_runtime_coordinator` 启动由外部 runtime coordinator 驱动的交互式 TUI。
pub fn run_with_runtime_coordinator(
    hero_options: HeroOptions,
    options: ModelOptions,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) -> Result<Model> {
    let mut model = Model::new_with_options(hero_options, options);

    if let Some(detection) = theme::try_detect_palette() {
        let _ = model.update(AppEvent::DetectedPalette {
            palette: detection.palette,
            has_dark_background: detection.has_dark_background,
        });
    }

    let (mut terminal, _guard) = TerminalSession::enter()?;
    let area = terminal.size()?;
    let _ = model.update(AppEvent::Resized {
        width: area.width,
        height: area.height,
    });

    let startup_deadline = Instant::now() + STARTUP_PROBE_TIMEOUT;
    let mut render_needed = true;
    let mut mouse_mode = TerminalMouseMode::for_mouse_capture(true);

    loop {
        render_needed |= drain_runtime_coordinator_events(&mut model, runtime_coordinator);

        if render_needed {
            terminal.draw(|frame| model.render(frame))?;
            // 覆盖层关闭 mouse capture 以保留原生选区，同时打开 alternate scroll，
            // 让终端把滚轮转成方向键交给 pager 处理。
            let desired_mouse_mode =
                TerminalMouseMode::for_mouse_capture(model.wants_mouse_capture());
            if desired_mouse_mode != mouse_mode {
                apply_mouse_mode(&mut terminal, desired_mouse_mode)?;
                mouse_mode = desired_mouse_mode;
            }
            render_needed = false;
        }

        if model.is_quitting() {
            break;
        }

        let now = Instant::now();
        if !model.has_palette() && now >= startup_deadline {
            let effect = model.update(AppEvent::StartupReadyTimeout);
            apply_effect_if_needed(&mut terminal, &mut model, runtime_coordinator, effect)?;
            render_needed = true;
            continue;
        }

        if let Some(timeout_event) = model.timeout_event(now) {
            let effect = model.update(timeout_event);
            apply_effect_if_needed(&mut terminal, &mut model, runtime_coordinator, effect)?;
            render_needed = true;
            continue;
        }

        let wait_plan = event_pipeline::terminal_wait_plan(
            &model,
            startup_deadline,
            now,
            runtime_coordinator.has_background_runtime(),
        );

        let first_event = match wait_for_terminal_event(wait_plan)? {
            Some(event) => event,
            None => {
                // timeout 到期或后台 runtime poll 到期。下一轮会先 drain runtime，
                // activity frame 到期时需要重绘；后台 poll 到期则只检查事件。
                render_needed = wait_plan.render_on_timeout();
                continue;
            }
        };

        let terminal_events = read_ready_terminal_events(first_event)?;
        for action in coalesced_input_actions(terminal_events) {
            match action {
                TerminalInputAction::App(app_event) => {
                    let effect = model.update(app_event);
                    apply_effect_if_needed(&mut terminal, &mut model, runtime_coordinator, effect)?;
                    render_needed = true;
                }
                TerminalInputAction::CancelExitConfirmation => {
                    model.cancel_exit_confirmation();
                    render_needed = true;
                }
            }

            if model.is_quitting() {
                break;
            }
        }
    }

    Ok(model)
}

fn drain_runtime_coordinator_events(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) -> bool {
    let mut changed = false;

    for event in runtime_coordinator.drain_runtime_events() {
        model.apply_runtime_event(event);
        changed = true;
    }

    for event in runtime_coordinator.drain_model_provider_refresh_events() {
        apply_model_provider_refresh_event(model, event);
        changed = true;
    }

    changed
}

#[cfg(test)]
mod tests;
