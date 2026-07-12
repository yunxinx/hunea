//! TUI runner 的事件循环与 runtime coordinator 边界。

use std::time::Instant;

use color_eyre::eyre::Result;
use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent},
};

use super::{
    AppEvent, Model, ModelOptions, STARTUP_PROBE_TIMEOUT, StartupBannerOptions, StyleMode,
    transcript::prewarm_markdown_highlighting,
};

mod conversation;
mod effects;
#[cfg(test)]
pub(crate) use effects::run_open_message_history_picker_effect;
mod event_pipeline;
mod external_io;
mod input;
mod loop_event_pump;
mod model_refresh;
mod terminal;
mod terminal_probe;
pub(crate) mod terminal_surface;

use super::{runtime::RuntimeEventApply, theme::palette_detection_from_background};
use effects::apply_effect_if_needed;
use external_io::{ExternalIoRuntime, apply_external_io_event};
pub(crate) use input::TerminalInputCoalescing;
use input::{TerminalInputAction, coalesced_input_actions_with_options};
pub use loop_event_pump::LoopEventWaker;
use loop_event_pump::{LoopEvent, LoopEventPump};
use model_refresh::apply_model_provider_refresh_event;
pub(crate) use terminal::TerminalMouseModePreference;
use terminal::{TerminalMouseMode, TerminalSession};

/// `RuntimeCoordinator` 是 TUI runner 与具体对话运行时之间的最小边界。
pub trait RuntimeCoordinator {
    fn install_loop_event_waker(
        &mut self,
        _waker: LoopEventWaker,
    ) -> std::result::Result<(), String> {
        Ok(())
    }

    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        Vec::new()
    }

    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        Vec::new()
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

    /// `begin_prompt_assembly_edit` 进入 `/prompt` overlay 时调用，加载 working copy，返回初始 snapshot。
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> std::result::Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String>
    {
        Err("Prompt assembly editing is not available".to_string())
    }

    /// `apply_prompt_assembly_edit_mutation` 在 working copy 上同步应用 mutation，返回刷新后的 snapshot。
    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        _mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> std::result::Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String>
    {
        Err("Prompt assembly editing is not available".to_string())
    }

    /// `commit_prompt_assembly_edit` 退出 `/prompt` overlay 时调用，diff baseline 决定是否落盘+通知。
    fn commit_prompt_assembly_edit(&mut self) -> std::result::Result<(), String> {
        Ok(())
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
    spawn_markdown_highlighting_prewarm();
    let mut model = Model::new_with_options(startup_banner_options, options);

    let (mut terminal, mut terminal_session) = TerminalSession::enter()?;

    let background_probe = terminal_probe::query_background(STARTUP_PROBE_TIMEOUT);
    apply_model_event_without_effect(
        &mut model,
        startup_palette_event(background_probe),
        "startup palette detection",
    );
    let area = terminal.size()?;
    apply_model_event_without_effect(
        &mut model,
        AppEvent::Resized {
            width: area.width,
            height: area.height,
        },
        "initial terminal resize",
    );

    let mut loop_events = LoopEventPump::start()?;
    runtime_coordinator
        .install_loop_event_waker(loop_events.waker())
        .map_err(color_eyre::eyre::Report::msg)?;

    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::LoadMessageHistoryStartupCache)
    {
        model.show_toast(crate::toast::ToastSeverity::Error, message);
    }
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::CheckPromptAssemblyMissingSources)
    {
        model.show_toast(crate::toast::ToastSeverity::Error, message);
    }

    let mut render_needed = true;
    let mut mouse_mode = TerminalMouseMode::for_mouse_capture(true);
    let mut external_io = ExternalIoRuntime::new(loop_events.waker());

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
                terminal_session.apply_mouse_mode(&mut terminal, desired_mouse_mode)?;
                mouse_mode = desired_mouse_mode;
            }
            render_needed = false;
        }

        if model.is_quitting() {
            break;
        }

        let now = Instant::now();
        if let Some(timeout_event) = model.timeout_event(now) {
            let effect = model.update(timeout_event);
            apply_effect_if_needed(
                &mut terminal,
                &mut terminal_session,
                &mut model,
                runtime_coordinator,
                &mut external_io,
                &mut loop_events,
                effect,
            )?;
            render_needed = true;
            continue;
        }

        let wait_plan = event_pipeline::loop_wait_plan(&model, now);
        let first_event = match loop_events.wait(wait_plan.timeout())? {
            Some(LoopEvent::Terminal(event)) => event,
            Some(LoopEvent::BackgroundReady) => continue,
            Some(LoopEvent::TerminalInputFailed(error)) => return Err(error.into()),
            None => {
                render_needed = wait_plan.render_on_timeout();
                continue;
            }
        };

        let input_coalescing = model.terminal_input_coalescing();
        let terminal_events = loop_events.collect_terminal_burst(first_event, input_coalescing)?;
        if apply_terminal_input_actions(
            coalesced_input_actions_with_options(terminal_events, input_coalescing),
            &mut terminal,
            &mut terminal_session,
            &mut model,
            runtime_coordinator,
            &mut external_io,
            &mut loop_events,
        )? {
            render_needed = true;
        }
    }

    apply_external_io_shutdown_events(&mut terminal, &mut model, &mut external_io)?;

    Ok(model)
}

fn spawn_markdown_highlighting_prewarm() {
    let _ = std::thread::Builder::new()
        .name("hunea-syntax-prewarm".to_string())
        .spawn(prewarm_markdown_highlighting);
}

fn startup_palette_event(
    background_probe: terminal_probe::TerminalBackgroundProbeResult,
) -> AppEvent {
    let Some(background) = background_probe.background else {
        return AppEvent::StartupReadyTimeout;
    };
    let detection = palette_detection_from_background(background);
    AppEvent::DetectedPalette {
        palette: detection.palette,
        has_dark_background: detection.has_dark_background,
    }
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
    terminal_session: &mut TerminalSession,
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    external_io: &mut ExternalIoRuntime,
    loop_events: &mut LoopEventPump,
) -> Result<bool> {
    let mut changed = false;
    for action in actions {
        match action {
            TerminalInputAction::App(app_event) => {
                let effect = model.update(app_event);
                apply_effect_if_needed(
                    terminal,
                    terminal_session,
                    model,
                    runtime_coordinator,
                    external_io,
                    loop_events,
                    effect,
                )?;
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
