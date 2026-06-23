//! TUI runner 的事件循环与 runtime coordinator 边界。

use std::time::Instant;

use color_eyre::eyre::Result;
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent},
};

use super::{
    AppEvent, Model, ModelOptions, STARTUP_PROBE_TIMEOUT, StartupBannerOptions, StyleMode, theme,
};

mod conversation;
mod effects;
#[cfg(test)]
pub(crate) use effects::run_open_message_history_picker_effect;
mod event_pipeline;
mod external_io;
mod input;
mod model_refresh;
mod terminal;
mod terminal_probe;
pub(crate) mod terminal_surface;

use super::{runtime::RuntimeEventApply, theme::palette_detection_from_background};
use effects::apply_effect_if_needed;
use external_io::{ExternalIoRuntime, apply_external_io_event};
pub(crate) use input::TerminalInputCoalescing;
use input::{
    TerminalInputAction, coalesced_input_actions_with_options, read_ready_terminal_events,
};
use model_refresh::apply_model_provider_refresh_event;
pub(crate) use terminal::TerminalMouseModePreference;
use terminal::{TerminalMouseMode, TerminalSession, apply_mouse_mode, wait_for_terminal_event};

/// `RuntimeCoordinator` 是 TUI runner 与具体对话运行时之间的最小边界。
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

impl RuntimeCoordinator for NoopRuntimeCoordinator {
    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> std::result::Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::LoadMessageHistoryStartupCache
            | RuntimeCommand::LoadMessageHistoryPickerRows { .. }
            | RuntimeCommand::RecordMessageHistory { .. } => Ok(RuntimeCommandReceipt::Accepted),
            _ => Err(match command.target() {
                Some(target) => format!("Runtime is not available: {}", target.display_label()),
                None => "Runtime is not available".to_string(),
            }),
        }
    }
}

/// `run` 启动交互式 TUI，并在退出后返回最终模型。
pub fn run(startup_banner_options: StartupBannerOptions) -> Result<Model> {
    run_with_options(startup_banner_options, ModelOptions::default())
}

/// `run_with_style_mode` 启动带指定样式模式的交互式 TUI。
pub fn run_with_style_mode(
    startup_banner_options: StartupBannerOptions,
    style_mode: StyleMode,
) -> Result<Model> {
    run_with_options(
        startup_banner_options,
        ModelOptions {
            style_mode,
            ..ModelOptions::default()
        },
    )
}

/// `run_with_options` 启动带显式选项的交互式 TUI。
pub fn run_with_options(
    startup_banner_options: StartupBannerOptions,
    options: ModelOptions,
) -> Result<Model> {
    let mut runtime_coordinator = NoopRuntimeCoordinator;
    run_with_runtime_coordinator(startup_banner_options, options, &mut runtime_coordinator)
}

/// `run_with_runtime_coordinator` 启动由外部 runtime coordinator 驱动的交互式 TUI。
pub fn run_with_runtime_coordinator(
    startup_banner_options: StartupBannerOptions,
    options: ModelOptions,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) -> Result<Model> {
    let mut model = Model::new_with_options(startup_banner_options, options);

    let (mut terminal, _guard) = TerminalSession::enter()?;

    let background_probe = terminal_probe::query_background(STARTUP_PROBE_TIMEOUT);
    if let Some(detection) = startup_palette_detection(background_probe) {
        apply_model_event_without_effect(
            &mut model,
            AppEvent::DetectedPalette {
                palette: detection.palette,
                has_dark_background: detection.has_dark_background,
            },
            "startup palette detection",
        );
    }
    let area = terminal.size()?;
    apply_model_event_without_effect(
        &mut model,
        AppEvent::Resized {
            width: area.width,
            height: area.height,
        },
        "initial terminal resize",
    );

    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::LoadMessageHistoryStartupCache)
    {
        model.show_toast(crate::toast::ToastSeverity::Error, message);
    }

    let startup_deadline = Instant::now() + STARTUP_PROBE_TIMEOUT;
    let mut render_needed = true;
    let mut mouse_mode = TerminalMouseMode::for_mouse_capture(true);
    let mut external_io = ExternalIoRuntime::new();

    loop {
        render_needed |= drain_runtime_coordinator_events(&mut model, runtime_coordinator);
        render_needed |= drain_external_io_events(&mut terminal, &mut model, &mut external_io)?;

        if render_needed {
            let frame_time = Instant::now();
            model.advance_toast_at(frame_time);
            terminal.draw(|area, buffer| model.render_to_buffer_at(frame_time, area, buffer))?;
            // 不同全屏界面对鼠标的需求不同：transcript 需要滚轮转方向键，
            // resume picker 则完全交还给终端以保留原生选区和滚动。
            let desired_mouse_mode =
                TerminalMouseMode::from_preference(model.mouse_mode_preference());
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
                runtime_coordinator,
                &mut external_io,
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
                runtime_coordinator,
                &mut external_io,
                effect,
            )?;
            render_needed = true;
            continue;
        }

        let has_background_work =
            runtime_coordinator.has_background_runtime() || external_io.has_pending_work();
        let wait_plan =
            event_pipeline::terminal_wait_plan(&model, startup_deadline, now, has_background_work);
        let first_event = match wait_for_terminal_event(wait_plan)? {
            Some(event) => event,
            None => {
                // timeout 到期或后台 runtime poll 到期。下一轮会先 drain runtime，
                // activity frame 到期时需要重绘；后台 poll 到期则只检查事件。
                render_needed = wait_plan.render_on_timeout();
                continue;
            }
        };

        let input_coalescing = model.terminal_input_coalescing();
        let terminal_events = read_ready_terminal_events(first_event, input_coalescing)?;
        if apply_terminal_input_actions(
            coalesced_input_actions_with_options(terminal_events, input_coalescing),
            &mut terminal,
            &mut model,
            runtime_coordinator,
            &mut external_io,
        )? {
            render_needed = true;
        }
    }

    apply_external_io_shutdown_events(&mut terminal, &mut model, &mut external_io)?;

    Ok(model)
}

fn startup_palette_detection(
    background_probe: terminal_probe::TerminalBackgroundProbeResult,
) -> Option<theme::PaletteDetection> {
    background_probe
        .background
        .map(palette_detection_from_background)
}

fn apply_model_event_without_effect(model: &mut Model, event: AppEvent, context: &str) {
    let effect = model.update(event);
    debug_assert!(
        effect.is_none(),
        "runner event `{context}` unexpectedly produced an AppEffect: {effect:?}"
    );
}

fn apply_terminal_input_actions(
    actions: Vec<TerminalInputAction>,
    terminal: &mut terminal::TuiTerminal,
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    external_io: &mut ExternalIoRuntime,
) -> Result<bool> {
    let mut changed = false;
    for action in actions {
        match action {
            TerminalInputAction::App(app_event) => {
                let effect = model.update(app_event);
                apply_effect_if_needed(terminal, model, runtime_coordinator, external_io, effect)?;
                changed = true;
            }
            TerminalInputAction::CancelExitConfirmation => {
                model.cancel_exit_confirmation();
                changed = true;
            }
        }

        if model.is_quitting() {
            break;
        }
    }

    Ok(changed)
}

fn drain_external_io_events(
    terminal: &mut terminal::TuiTerminal,
    model: &mut Model,
    external_io: &mut ExternalIoRuntime,
) -> Result<bool> {
    let events = external_io.drain_events();
    if events.is_empty() {
        return Ok(false);
    }

    for event in events {
        apply_external_io_event(terminal, model, event)?;
    }

    Ok(true)
}

fn apply_external_io_shutdown_events(
    terminal: &mut terminal::TuiTerminal,
    model: &mut Model,
    external_io: &mut ExternalIoRuntime,
) -> Result<()> {
    for event in external_io.shutdown_and_drain_events() {
        apply_external_io_event(terminal, model, event)?;
    }
    Ok(())
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
