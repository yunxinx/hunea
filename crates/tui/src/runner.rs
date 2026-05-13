use std::{path::PathBuf, time::Instant};

use ::mo_acp::AcpSessionCatalog;
use ::mo_native_agent::{ModelProviderRefreshRuntimeState, NativeAgentRuntimeState};
use color_eyre::eyre::Result;
use mo_core::request_policy::RuntimeRequestPolicy;

use super::{AppEvent, HeroOptions, Model, ModelOptions, STARTUP_PROBE_TIMEOUT, StyleMode, theme};

mod acp_session;
mod effects;
mod event_pipeline;
mod external_io;
mod input;
mod model_refresh;
mod native_agent;
mod terminal;

use self::native_agent::drain_native_agent_runtime_events;
use acp_session::{AcpRuntimeState, drain_acp_runtime_events};
use effects::apply_effect_if_needed;
use input::{TerminalInputAction, coalesced_input_actions, read_ready_terminal_events};
use model_refresh::drain_model_refresh_runtime_events;
use terminal::{TerminalMouseMode, TerminalSession, apply_mouse_mode, wait_for_terminal_event};

/// `RuntimeOptions` 表示 TUI runner 可执行的外部 runtime 能力。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub acp_sessions: AcpSessionCatalog,
    pub model_config_path: Option<PathBuf>,
    pub runtime_request_policy: RuntimeRequestPolicy,
}

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
    run_with_runtime_options(hero_options, options, RuntimeOptions::default())
}

/// `run_with_runtime_options` 启动带显式 runtime 能力的交互式 TUI。
pub fn run_with_runtime_options(
    hero_options: HeroOptions,
    options: ModelOptions,
    runtime_options: RuntimeOptions,
) -> Result<Model> {
    let mut model = Model::new_with_options(hero_options, options);
    let mut acp_runtime = AcpRuntimeState::default();
    let mut native_agent_runtime = NativeAgentRuntimeState::default();
    let mut model_refresh_runtime = ModelProviderRefreshRuntimeState::default();

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
        render_needed |= drain_acp_runtime_events(&mut model, &mut acp_runtime);
        render_needed |= drain_native_agent_runtime_events(&mut model, &mut native_agent_runtime);
        render_needed |= drain_model_refresh_runtime_events(&mut model, &mut model_refresh_runtime);

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
            apply_effect_if_needed(
                &mut terminal,
                &mut model,
                &runtime_options,
                &mut acp_runtime,
                &mut native_agent_runtime,
                &mut model_refresh_runtime,
                effect,
            )?;
            render_needed = true;
            continue;
        }

        if let Some(timeout_event) = model.timeout_event(now) {
            let effect = model.update(timeout_event);
            apply_effect_if_needed(
                &mut terminal,
                &mut model,
                &runtime_options,
                &mut acp_runtime,
                &mut native_agent_runtime,
                &mut model_refresh_runtime,
                effect,
            )?;
            render_needed = true;
            continue;
        }

        let wait_plan = event_pipeline::terminal_wait_plan(
            &model,
            startup_deadline,
            now,
            has_background_runtime(&acp_runtime, &native_agent_runtime, &model_refresh_runtime),
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
                    apply_effect_if_needed(
                        &mut terminal,
                        &mut model,
                        &runtime_options,
                        &mut acp_runtime,
                        &mut native_agent_runtime,
                        &mut model_refresh_runtime,
                        effect,
                    )?;
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

fn has_background_runtime(
    acp_runtime: &AcpRuntimeState,
    native_agent_runtime: &NativeAgentRuntimeState,
    model_refresh_runtime: &ModelProviderRefreshRuntimeState,
) -> bool {
    acp_runtime.should_poll_events()
        || native_agent_runtime.is_running()
        || model_refresh_runtime.is_running()
}

#[cfg(test)]
mod tests;
