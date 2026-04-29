use std::{
    io,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use crate::runtime::acp::{
    AcpInitializeOutcome, AcpSessionCatalog, AcpSessionCommand, AcpSessionEvent, AcpSessionWorker,
};
use crate::runtime::llm::{
    CancellationToken, ChatPerformanceMetrics, LlmError, NativeChatProgress, NativeChatRequest,
    NativeChatResponse, send_chat_with_cancellation_and_token_progress,
};
use crate::runtime::token_count::StreamingTokenProgress;
use crate::runtime::{
    models,
    models::{ModelSelection, ProviderSyncRequest},
};
use arboard::Clipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use color_eyre::eyre::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

use super::{
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, RequestMetrics, STARTUP_PROBE_TIMEOUT,
    StyleMode, theme,
};

mod event_pipeline;
use event_pipeline::TerminalWaitPlan;

/// `RuntimeOptions` 表示 TUI runner 可执行的外部 runtime 能力。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub acp_sessions: AcpSessionCatalog,
    pub model_config_path: Option<PathBuf>,
    pub runtime_request_policy: RuntimeRequestPolicy,
}

/// `RuntimeRequestPolicy` 描述交互式 runtime 请求的超时与重试策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequestPolicy {
    attempts: usize,
    delays: Vec<Duration>,
    timeout: Duration,
}

impl RuntimeRequestPolicy {
    pub fn new(attempts: usize, delays_seconds: Vec<u64>, timeout_seconds: u64) -> Self {
        Self {
            attempts,
            delays: delays_seconds
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            timeout: Duration::from_secs(timeout_seconds),
        }
    }

    pub(crate) fn attempts(&self) -> usize {
        self.attempts
    }

    pub(crate) fn delay_for_retry(&self, retry: usize) -> Duration {
        self.delays
            .get(retry.saturating_sub(1))
            .copied()
            .or_else(|| self.delays.last().copied())
            .unwrap_or_else(|| Duration::from_secs(1))
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for RuntimeRequestPolicy {
    fn default() -> Self {
        Self::new(3, vec![1, 2, 3], 120)
    }
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
    let mut native_chat_runtime = NativeChatRuntimeState::default();
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

    loop {
        render_needed |= drain_acp_runtime_events(&mut model, &mut acp_runtime);
        render_needed |= drain_native_chat_runtime_events(&mut model, &mut native_chat_runtime);
        render_needed |= drain_model_refresh_runtime_events(&mut model, &mut model_refresh_runtime);

        if render_needed {
            terminal.draw(|frame| model.render(frame))?;
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
                &mut native_chat_runtime,
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
                &mut native_chat_runtime,
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
            has_background_runtime(&acp_runtime, &native_chat_runtime, &model_refresh_runtime),
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
                        &mut native_chat_runtime,
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

fn wait_for_terminal_event(wait_plan: TerminalWaitPlan) -> io::Result<Option<Event>> {
    match wait_plan {
        TerminalWaitPlan::Block => event::read().map(Some),
        TerminalWaitPlan::Poll { duration, .. } => {
            if event::poll(duration)? {
                event::read().map(Some)
            } else {
                Ok(None)
            }
        }
    }
}

fn has_background_runtime(
    acp_runtime: &AcpRuntimeState,
    native_chat_runtime: &NativeChatRuntimeState,
    model_refresh_runtime: &ModelProviderRefreshRuntimeState,
) -> bool {
    acp_runtime.should_poll_events()
        || native_chat_runtime.is_running()
        || model_refresh_runtime.is_running()
}

struct TerminalSession;

impl TerminalSession {
    fn enter() -> io::Result<(Terminal<CrosstermBackend<io::Stdout>>, Self)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
        )?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        Ok((terminal, Self))
    }

    fn suspend(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        terminal.show_cursor()?;
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        )?;
        Ok(())
    }

    fn resume(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        enable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            Hide
        )?;
        terminal.hide_cursor()?;
        terminal.clear()?;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            Show,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
    }
}

const MAX_READY_TERMINAL_EVENTS_PER_FRAME: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalInputAction {
    App(AppEvent),
    CancelExitConfirmation,
}

fn read_ready_terminal_events(first_event: Event) -> Result<Vec<Event>> {
    let mut events = vec![first_event];
    while events.len() < MAX_READY_TERMINAL_EVENTS_PER_FRAME && event::poll(Duration::ZERO)? {
        events.push(event::read()?);
    }
    Ok(events)
}

fn coalesced_input_actions(events: impl IntoIterator<Item = Event>) -> Vec<TerminalInputAction> {
    let mut actions = Vec::new();
    let mut pending_wheel_delta = 0_isize;

    for event in events {
        match event {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    pending_wheel_delta -= Model::document_mouse_wheel_delta();
                }
                MouseEventKind::ScrollDown => {
                    pending_wheel_delta += Model::document_mouse_wheel_delta();
                }
                MouseEventKind::Down(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseDown {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                MouseEventKind::Up(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseUp {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                MouseEventKind::Drag(button) => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::App(AppEvent::MouseDrag {
                        button,
                        column: mouse.column,
                        row: mouse.row,
                    }));
                }
                _ => {
                    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                    actions.push(TerminalInputAction::CancelExitConfirmation);
                }
            },
            Event::Key(key) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Key(key)));
            }
            Event::Paste(text) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Paste(text)));
            }
            Event::Resize(width, height) => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
                actions.push(TerminalInputAction::App(AppEvent::Resized {
                    width,
                    height,
                }));
            }
            _ => {
                flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
            }
        }
    }

    flush_pending_wheel_delta(&mut actions, &mut pending_wheel_delta);
    actions
}

fn flush_pending_wheel_delta(actions: &mut Vec<TerminalInputAction>, delta: &mut isize) {
    if *delta == 0 {
        return;
    }

    actions.push(TerminalInputAction::App(AppEvent::MouseWheel {
        delta_lines: *delta,
    }));
    *delta = 0;
}

fn apply_effect_if_needed(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Model,
    runtime_options: &RuntimeOptions,
    acp_runtime: &mut AcpRuntimeState,
    native_chat_runtime: &mut NativeChatRuntimeState,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
    effect: Option<AppEffect>,
) -> Result<()> {
    let Some(effect) = effect else {
        return Ok(());
    };

    match effect {
        AppEffect::LaunchExternalEditor(launch) => {
            run_external_editor_effect(terminal, model, launch)
        }
        AppEffect::CopySelection(text) => run_copy_selection_effect(terminal, model, &text),
        AppEffect::ResetRuntimeSession => {
            reset_runtime_session_after_clear(
                acp_runtime,
                native_chat_runtime,
                model_refresh_runtime,
            );
            Ok(())
        }
        AppEffect::StartAcpSession { agent_id } => {
            run_start_acp_session_effect(model, runtime_options, acp_runtime, &agent_id)
        }
        AppEffect::SendAcpPrompt { agent_id, prompt } => {
            run_send_acp_prompt_effect(model, acp_runtime, &agent_id, prompt);
            Ok(())
        }
        AppEffect::RespondAcpPermission {
            request_id,
            option_id,
        } => {
            run_respond_acp_permission_effect(model, acp_runtime, &request_id, option_id);
            Ok(())
        }
        AppEffect::SetAcpModel { config_id, value } => {
            run_set_acp_model_effect(model, acp_runtime, config_id, value);
            Ok(())
        }
        AppEffect::PersistSelectedModel { selection } => {
            run_persist_selected_model_effect(model, runtime_options, &selection);
            Ok(())
        }
        AppEffect::RefreshModelProvider { request } => {
            run_refresh_model_provider_effect(model, model_refresh_runtime, request);
            Ok(())
        }
        AppEffect::SendNativeChat { request } => {
            run_send_native_chat_effect(
                model,
                native_chat_runtime,
                request,
                runtime_options.runtime_request_policy.clone(),
            );
            Ok(())
        }
        AppEffect::InterruptCurrentTurn => {
            run_interrupt_current_turn_effect(model, acp_runtime, native_chat_runtime);
            Ok(())
        }
    }
}

fn run_persist_selected_model_effect(
    model: &mut Model,
    runtime_options: &RuntimeOptions,
    selection: &ModelSelection,
) {
    if let Err(error) =
        models::write_default_model(runtime_options.model_config_path.as_deref(), selection)
    {
        model.show_transient_status_notice(&format!("Failed to save default model: {error}"));
    }
}

fn reset_runtime_session_after_clear(
    acp_runtime: &mut AcpRuntimeState,
    native_chat_runtime: &mut NativeChatRuntimeState,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
) {
    acp_runtime.reset_after_clear();
    native_chat_runtime.reset_after_clear();
    model_refresh_runtime.reset_after_clear();
}

#[derive(Default)]
struct ModelProviderRefreshRuntimeState {
    receiver: Option<Receiver<ModelProviderRefreshEvent>>,
}

impl ModelProviderRefreshRuntimeState {
    fn start(&mut self, request: ProviderSyncRequest) {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let provider_id = request.provider_id.clone();
            let event = match models::sync_provider_models_once(&request) {
                Ok(model_ids) => ModelProviderRefreshEvent::Finished {
                    provider_id,
                    model_ids,
                },
                Err(message) => ModelProviderRefreshEvent::Failed {
                    provider_id,
                    message,
                },
            };
            let _ = sender.send(event);
        });
        self.receiver = Some(receiver);
    }

    fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    fn reset_after_clear(&mut self) {
        self.receiver = None;
    }

    fn try_recv_event(&mut self) -> Option<ModelProviderRefreshEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                self.receiver = None;
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Some(ModelProviderRefreshEvent::Failed {
                    provider_id: String::new(),
                    message: "model refresh stopped before completion".to_string(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelProviderRefreshEvent {
    Finished {
        provider_id: String,
        model_ids: Vec<String>,
    },
    Failed {
        provider_id: String,
        message: String,
    },
}

fn drain_model_refresh_runtime_events(
    model: &mut Model,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
) -> bool {
    let mut changed = false;
    while let Some(event) = model_refresh_runtime.try_recv_event() {
        apply_model_provider_refresh_event(model, event);
        changed = true;
    }
    changed
}

fn apply_model_provider_refresh_event(model: &mut Model, event: ModelProviderRefreshEvent) {
    match event {
        ModelProviderRefreshEvent::Finished {
            provider_id,
            model_ids,
        } => model.apply_model_provider_refresh_success(&provider_id, model_ids),
        ModelProviderRefreshEvent::Failed {
            provider_id,
            message,
        } => model.apply_model_provider_refresh_failure(&provider_id, message),
    }
}

fn run_refresh_model_provider_effect(
    model: &mut Model,
    model_refresh_runtime: &mut ModelProviderRefreshRuntimeState,
    request: ProviderSyncRequest,
) {
    if model_refresh_runtime.is_running() {
        model.show_transient_status_notice("Model refresh is already running");
        return;
    }

    model_refresh_runtime.start(request);
}

#[derive(Default)]
struct NativeChatRuntimeState {
    receiver: Option<Receiver<NativeChatEvent>>,
    cancellation: Option<CancellationToken>,
}

impl NativeChatRuntimeState {
    fn start(&mut self, request: NativeChatRequest, request_policy: RuntimeRequestPolicy) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run_native_chat_worker(
                        request,
                        request_policy,
                        thread_cancellation,
                        sender,
                    ));
                }
                Err(error) => {
                    let _ = sender.send(NativeChatEvent::Failed {
                        message: format!("start chat runtime: {error}"),
                    });
                }
            }
        });
        self.receiver = Some(receiver);
        self.cancellation = Some(cancellation);
    }

    fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    fn reset_after_clear(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.receiver = None;
    }

    fn interrupt(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.receiver = None;
        true
    }

    fn try_recv_event(&mut self) -> Option<NativeChatEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                if event.is_terminal() {
                    self.receiver = None;
                    self.cancellation = None;
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.cancellation = None;
                Some(NativeChatEvent::Failed {
                    message: "chat request stopped before completion".to_string(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NativeChatEvent {
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    Finished {
        response: NativeChatResponse,
        metrics: Option<ChatPerformanceMetrics>,
    },
    Failed {
        message: String,
    },
    Interrupted,
}

impl NativeChatEvent {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Interrupted
        )
    }
}

async fn run_native_chat_worker(
    request: NativeChatRequest,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    sender: mpsc::Sender<NativeChatEvent>,
) {
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let attempt_result = tokio::time::timeout(
            request_policy.timeout(),
            send_chat_with_cancellation_and_token_progress(
                &request,
                &cancellation,
                move |progress| {
                    let event = match progress {
                        NativeChatProgress::OutputTokens { total_tokens } => {
                            NativeChatEvent::OutputTokenEstimate { total_tokens }
                        }
                        NativeChatProgress::Thinking { is_thinking } => {
                            NativeChatEvent::Thinking { is_thinking }
                        }
                    };
                    let _ = progress_sender.send(event);
                },
            ),
        )
        .await;

        match attempt_result {
            Err(_elapsed) if attempt < request_policy.attempts() => {
                if retry_native_chat_after_attempt(attempt, &request_policy, &cancellation, &sender)
                    .await
                {
                    return;
                }
            }
            Err(_elapsed) => {
                let _ = sender.send(NativeChatEvent::Failed {
                    message: format!(
                        "Chat request timed out after {}s",
                        request_policy.timeout().as_secs()
                    ),
                });
                return;
            }
            Ok(Ok(completion)) => {
                let _ = sender.send(NativeChatEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                });
                return;
            }
            Ok(Err(LlmError::Cancelled)) => {
                let _ = sender.send(NativeChatEvent::Interrupted);
                return;
            }
            Ok(Err(_error)) if attempt < request_policy.attempts() => {
                if retry_native_chat_after_attempt(attempt, &request_policy, &cancellation, &sender)
                    .await
                {
                    return;
                }
            }
            Ok(Err(error)) => {
                let _ = sender.send(NativeChatEvent::Failed {
                    message: error.to_string(),
                });
                return;
            }
        }
    }
}

fn drain_native_chat_runtime_events(
    model: &mut Model,
    native_chat_runtime: &mut NativeChatRuntimeState,
) -> bool {
    let mut changed = false;
    while let Some(event) = native_chat_runtime.try_recv_event() {
        apply_native_chat_event(model, event);
        changed = true;
    }
    changed
}

fn apply_native_chat_event(model: &mut Model, event: NativeChatEvent) {
    match event {
        NativeChatEvent::Retrying { message } => {
            model.show_acp_activity_with_header(message);
        }
        NativeChatEvent::OutputTokenEstimate { total_tokens } => {
            model.set_acp_activity_output_tokens(total_tokens);
        }
        NativeChatEvent::Thinking { is_thinking } => {
            model.set_acp_activity_thinking(is_thinking);
        }
        NativeChatEvent::Finished { response, metrics } => {
            if let Some(metrics) = metrics {
                model.set_last_request_metrics(Some(RequestMetrics::new(
                    metrics.latency,
                    metrics.output_tokens,
                    metrics.duration,
                )));
            }
            model.clear_acp_activity();
            model.append_native_chat_response_from_runtime(response);
        }
        NativeChatEvent::Failed { message } => {
            model.clear_acp_activity();
            model.append_system_message_from_runtime(format!("Chat failed: {message}"));
        }
        NativeChatEvent::Interrupted => {
            model.clear_acp_activity();
            model.append_system_message_from_runtime("Chat interrupted");
        }
    }
}

async fn retry_native_chat_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<NativeChatEvent>,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(NativeChatEvent::Retrying {
        message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
    });
    tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = sender.send(NativeChatEvent::Interrupted);
            true
        }
        _ = tokio::time::sleep(request_policy.delay_for_retry(retry)) => false,
    }
}

fn run_send_native_chat_effect(
    model: &mut Model,
    native_chat_runtime: &mut NativeChatRuntimeState,
    request: NativeChatRequest,
    request_policy: RuntimeRequestPolicy,
) {
    if native_chat_runtime.is_running() {
        model.show_transient_status_notice("Chat request is already running");
        return;
    }

    let activity_label = request.model_id.clone();
    native_chat_runtime.start(request, request_policy);
    model.show_acp_activity(activity_label);
}

fn run_interrupt_native_chat_effect(
    model: &mut Model,
    native_chat_runtime: &mut NativeChatRuntimeState,
) -> bool {
    if native_chat_runtime.interrupt() {
        model.clear_acp_activity();
        model.append_system_message_from_runtime("Chat interrupted");
        return true;
    }
    false
}

fn run_interrupt_current_turn_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    native_chat_runtime: &mut NativeChatRuntimeState,
) {
    if run_interrupt_native_chat_effect(model, native_chat_runtime) {
        return;
    }

    run_interrupt_acp_prompt_effect(model, acp_runtime);
}

fn run_interrupt_acp_prompt_effect(model: &mut Model, acp_runtime: &mut AcpRuntimeState) {
    if !acp_runtime.interrupt_prompt() {
        return;
    }

    if let Some(pending) = model.pending_acp_permission.take() {
        let _ = acp_runtime.respond_permission(&pending.request_id, None);
        model.close_tool_approval_panel();
    }
    model.clear_acp_activity();
    model.append_system_message_from_runtime("Chat interrupted");
}

#[derive(Default)]
struct AcpRuntimeState {
    worker: Option<AcpSessionWorker>,
    response_buffer: String,
    reasoning_buffer: String,
    reasoning_started_at: Option<Instant>,
    prompt_in_flight: bool,
    discard_in_flight_prompt: bool,
    token_progress: Option<StreamingTokenProgress>,
    prompt_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
}

impl AcpRuntimeState {
    fn should_poll_events(&self) -> bool {
        self.worker.is_some()
    }

    fn start(&mut self, command: AcpSessionCommand) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.prompt_in_flight = false;
        self.discard_in_flight_prompt = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.worker = Some(AcpSessionWorker::start(command));
    }

    fn reset_response_buffer(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
    }

    fn push_response_chunk(&mut self, content: &str) {
        if !content.is_empty() {
            self.first_token_at.get_or_insert_with(Instant::now);
        }
        self.response_buffer.push_str(content);
    }

    fn push_reasoning_chunk(&mut self, content: &str) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert_with(Instant::now);
        if self.reasoning_started_at.is_none() {
            self.reasoning_started_at = Some(Instant::now());
        }
        self.reasoning_buffer.push_str(content);
    }

    fn take_response_buffer(&mut self) -> Option<String> {
        if self.response_buffer.is_empty() {
            return None;
        }

        Some(std::mem::take(&mut self.response_buffer))
    }

    fn take_reasoning_buffer(&mut self) -> (Option<String>, Option<Duration>) {
        if self.reasoning_buffer.is_empty() {
            self.reasoning_started_at = None;
            return (None, None);
        }

        let duration = self
            .reasoning_started_at
            .take()
            .map(|started_at| Instant::now().saturating_duration_since(started_at));
        (Some(std::mem::take(&mut self.reasoning_buffer)), duration)
    }

    fn mark_prompt_submitted(&mut self) {
        self.prompt_in_flight = true;
    }

    fn mark_prompt_started(&mut self) {
        self.prompt_in_flight = true;
        self.prompt_started_at = Some(Instant::now());
        self.first_token_at = None;
    }

    fn start_token_progress(&mut self, model_id: impl Into<String>) {
        self.token_progress = Some(StreamingTokenProgress::new(model_id));
    }

    fn observe_output_tokens(&mut self, content: &str) -> Option<usize> {
        self.token_progress
            .as_mut()
            .and_then(|progress| progress.observe_delta(content, Instant::now()))
    }

    fn flush_output_tokens(&mut self) -> Option<usize> {
        self.token_progress
            .as_mut()
            .and_then(|progress| progress.flush(Instant::now()))
    }

    fn total_output_tokens(&self) -> usize {
        self.token_progress
            .as_ref()
            .map(StreamingTokenProgress::total_tokens)
            .unwrap_or(0)
    }

    fn request_metrics(&self, finished_at: Instant) -> Option<RequestMetrics> {
        let prompt_started_at = self.prompt_started_at?;
        let first_token_at = self.first_token_at?;
        Some(RequestMetrics::new(
            first_token_at.saturating_duration_since(prompt_started_at),
            self.total_output_tokens(),
            finished_at.saturating_duration_since(prompt_started_at),
        ))
    }

    fn mark_prompt_finished(&mut self) {
        self.prompt_in_flight = false;
        self.discard_in_flight_prompt = false;
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
    }

    fn should_discard_prompt_output(&self) -> bool {
        self.discard_in_flight_prompt
    }

    fn interrupt_prompt(&mut self) -> bool {
        if !self.prompt_in_flight {
            return false;
        }
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.discard_in_flight_prompt = true;
        if let Some(worker) = self.worker.as_ref() {
            let _ = worker.cancel_prompt();
        }
        true
    }

    fn reset_after_clear(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        if self.prompt_in_flight {
            self.discard_in_flight_prompt = true;
        }
    }

    fn send_prompt(&self, agent_id: &str, prompt: String) -> Result<(), String> {
        let Some(worker) = self.worker.as_ref() else {
            return Err(format!("ACP session is not ready: {agent_id}"));
        };
        if worker.agent_id() != agent_id {
            return Err(format!("ACP session is not active: {agent_id}"));
        }

        worker
            .send_prompt(prompt)
            .map_err(|error| error.to_string())
    }

    fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let Some(worker) = self.worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker
            .respond_permission(request_id, option_id)
            .map_err(|error| error.to_string())
    }

    fn set_config_option(&self, config_id: String, value: String) -> Result<(), String> {
        let Some(worker) = self.worker.as_ref() else {
            return Err("ACP session is not ready".to_string());
        };

        worker
            .set_config_option(config_id, value)
            .map_err(|error| error.to_string())
    }
}

fn drain_acp_runtime_events(model: &mut Model, acp_runtime: &mut AcpRuntimeState) -> bool {
    let Some(worker) = acp_runtime.worker.as_ref() else {
        return false;
    };

    let mut events = Vec::new();
    while let Some(event) = worker.try_recv_event() {
        events.push(event);
    }

    let changed = !events.is_empty();
    for event in events {
        apply_acp_session_event(model, acp_runtime, event);
    }
    changed
}

fn apply_acp_session_event(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    event: AcpSessionEvent,
) {
    match event {
        AcpSessionEvent::Started { outcome, .. } => {
            model.show_transient_status_notice(&format!(
                "ACP session ready: {}",
                acp_agent_display_name(&outcome)
            ));
        }
        AcpSessionEvent::StartFailed { message, .. } => {
            model.show_transient_status_notice(&format!("ACP start failed: {message}"));
        }
        AcpSessionEvent::PromptStarted { agent_id } => {
            acp_runtime.reset_response_buffer();
            acp_runtime.mark_prompt_started();
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            acp_runtime.start_token_progress(
                model
                    .acp_current_model
                    .clone()
                    .unwrap_or_else(|| agent_id.clone()),
            );
            if model.acp_activity.is_none() {
                model.show_acp_activity(agent_id);
            }
        }
        AcpSessionEvent::AgentThoughtChunk { content, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            acp_runtime.push_reasoning_chunk(&content);
            model.set_acp_activity_thinking(true);
            if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                model.set_acp_activity_output_tokens(total_tokens);
            }
        }
        AcpSessionEvent::AgentMessageChunk { content, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            model.set_acp_activity_thinking(false);
            acp_runtime.push_response_chunk(&content);
            if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                model.set_acp_activity_output_tokens(total_tokens);
            }
        }
        AcpSessionEvent::ModelConfigChanged { agent_id, config } => {
            model.apply_acp_model_config(&agent_id, config);
        }
        AcpSessionEvent::ConfigChangeFailed { message, .. } => {
            model.show_transient_status_notice(&format!("ACP config change failed: {message}"));
        }
        AcpSessionEvent::PromptResponse {
            content,
            stop_reason,
            ..
        } => {
            if acp_runtime.should_discard_prompt_output() {
                acp_runtime.mark_prompt_finished();
                model.clear_acp_activity();
                return;
            }
            if !content.is_empty() {
                acp_runtime.push_response_chunk(&content);
                if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                    model.set_acp_activity_output_tokens(total_tokens);
                }
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_acp_activity_output_tokens(total_tokens);
            }
            let metrics = acp_runtime.request_metrics(Instant::now());
            model.set_acp_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            if let Some(metrics) = metrics {
                model.set_last_request_metrics(Some(metrics));
            }
            acp_runtime.mark_prompt_finished();
            model.clear_acp_activity();
            if stop_reason != "EndTurn" {
                model.show_transient_status_notice(&format!("ACP prompt finished: {stop_reason}"));
            }
        }
        AcpSessionEvent::PromptFailed { message, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                acp_runtime.mark_prompt_finished();
                model.clear_acp_activity();
                return;
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_acp_activity_output_tokens(total_tokens);
            }
            model.set_acp_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            acp_runtime.mark_prompt_finished();
            model.clear_acp_activity();
            model.show_transient_status_notice(&format!("ACP prompt failed: {message}"));
        }
        AcpSessionEvent::PromptInterrupted { .. } => {
            acp_runtime.mark_prompt_finished();
            model.clear_acp_activity();
        }
        AcpSessionEvent::PermissionRequested { request, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                let options = acp_permission_option_ids(&request);
                let _ = acp_runtime
                    .respond_permission(&request.request_id, options.reject_for_cancel());
                return;
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_acp_activity_output_tokens(total_tokens);
            }
            model.set_acp_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            let options = acp_permission_option_ids(&request);
            model.update(AppEvent::AcpPermissionRequested {
                request_id: request.request_id,
                title: request.title,
                allow_option_id: options.allow_once,
                allow_always_option_id: options.allow_always,
                reject_option_id: options.reject,
                reject_always_option_id: options.reject_always,
            });
        }
        AcpSessionEvent::PermissionRequestCancelled { .. } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            model.close_tool_approval_panel();
            model.show_transient_status_notice("ACP permission request cancelled");
        }
        AcpSessionEvent::Stopped { message, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                acp_runtime.mark_prompt_finished();
                model.clear_acp_activity();
                return;
            }
            flush_acp_response_buffer(model, acp_runtime);
            model.clear_acp_activity();
            if let Some(message) = message {
                model.show_transient_status_notice(&format!("ACP session stopped: {message}"));
            }
        }
    }
}

fn flush_acp_response_buffer(model: &mut Model, acp_runtime: &mut AcpRuntimeState) {
    let content = acp_runtime.take_response_buffer();
    let (reasoning_content, reasoning_duration) = acp_runtime.take_reasoning_buffer();
    if content.is_some() || reasoning_content.is_some() {
        model.append_acp_response_from_runtime(
            content.unwrap_or_default(),
            reasoning_content,
            reasoning_duration,
        );
    }
}

fn run_start_acp_session_effect(
    model: &mut Model,
    runtime_options: &RuntimeOptions,
    acp_runtime: &mut AcpRuntimeState,
    agent_id: &str,
) -> Result<()> {
    let Some(command) = runtime_options.acp_sessions.command(agent_id) else {
        model.show_transient_status_notice(&format!(
            "ACP agent needs installation before starting: {agent_id}"
        ));
        return Ok(());
    };

    acp_runtime.start(command.clone());
    model.set_acp_current_model(command.default_model.clone());
    model.show_transient_status_notice(&format!("Starting ACP agent: {agent_id}"));
    Ok(())
}

fn run_send_acp_prompt_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    agent_id: &str,
    prompt: String,
) {
    if let Err(message) = acp_runtime.send_prompt(agent_id, prompt) {
        model.clear_acp_activity();
        model.show_transient_status_notice(&message);
    } else {
        acp_runtime.mark_prompt_submitted();
    }
}

fn run_respond_acp_permission_effect(
    model: &mut Model,
    acp_runtime: &AcpRuntimeState,
    request_id: &str,
    option_id: Option<String>,
) {
    if let Err(message) = acp_runtime.respond_permission(request_id, option_id) {
        model.show_transient_status_notice(&message);
    }
}

fn run_set_acp_model_effect(
    model: &mut Model,
    acp_runtime: &AcpRuntimeState,
    config_id: String,
    value: String,
) {
    if let Err(message) = acp_runtime.set_config_option(config_id, value) {
        model.show_transient_status_notice(&message);
    }
}

struct AcpPermissionOptionIds {
    allow_once: Option<String>,
    allow_always: Option<String>,
    reject: Option<String>,
    reject_always: Option<String>,
}

impl AcpPermissionOptionIds {
    fn reject_for_cancel(&self) -> Option<String> {
        self.reject.clone().or_else(|| self.reject_always.clone())
    }
}

fn acp_permission_option_ids(
    request: &crate::runtime::acp::AcpPermissionRequest,
) -> AcpPermissionOptionIds {
    use crate::runtime::acp::AcpPermissionOptionKind;

    let allow_once = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::AllowOnce)
        .map(|option| option.option_id.clone());

    let allow_always = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::AllowAlways)
        .map(|option| option.option_id.clone());

    let reject = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::RejectOnce)
        .map(|option| option.option_id.clone());

    let reject_always = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::RejectAlways)
        .map(|option| option.option_id.clone());

    AcpPermissionOptionIds {
        allow_once,
        allow_always,
        reject,
        reject_always,
    }
}

fn acp_agent_display_name(outcome: &AcpInitializeOutcome) -> String {
    outcome
        .agent_title
        .as_deref()
        .or(outcome.agent_name.as_deref())
        .unwrap_or("unknown agent")
        .to_string()
}

fn run_external_editor_effect(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Model,
    launch: super::ExternalEditorLaunch,
) -> Result<()> {
    TerminalSession::suspend(terminal)?;
    let failed = run_external_editor_command(&launch.command).is_err();
    TerminalSession::resume(terminal)?;

    let area = terminal.size()?;
    let _ = model.update(AppEvent::Resized {
        width: area.width,
        height: area.height,
    });
    let _ = model.update(AppEvent::ExternalEditorFinished {
        draft_path: launch.draft_path,
        original_draft: launch.original_draft,
        failed,
    });
    Ok(())
}

fn run_copy_selection_effect(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Model,
    text: &str,
) -> Result<()> {
    let copied = copy_selection_to_system_or_terminal_clipboard(terminal, text);
    let _ = model.update(AppEvent::SelectionCopyCompleted { success: copied });
    Ok(())
}

fn run_external_editor_command(command: &[String]) -> io::Result<()> {
    if command.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "external editor command is empty",
        ));
    }

    let status = Command::new(&command[0]).args(&command[1..]).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            "external editor exited with a failure status",
        ))
    }
}

fn copy_selection_to_system_or_terminal_clipboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    text: &str,
) -> bool {
    if copy_selection_to_system_clipboard(text).is_ok() {
        return true;
    }

    copy_selection_to_terminal_clipboard(terminal, text).is_ok()
}

fn copy_selection_to_system_clipboard(text: &str) -> Result<(), arboard::Error> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text.to_string())
}

fn copy_selection_to_terminal_clipboard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    text: &str,
) -> io::Result<()> {
    use std::io::Write as _;

    let encoded = BASE64_STANDARD.encode(text.as_bytes());
    let sequence = format!("\u{1b}]52;c;{encoded}\u{7}");
    terminal.backend_mut().write_all(sequence.as_bytes())?;
    terminal.backend_mut().flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::tui::{ReasoningDisplayMode, Sender, StatusLineItem};
    use crate::runtime::models::ModelSelection;
    use crate::runtime::phrases::StatusPhraseOrder;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    #[test]
    fn acp_chunks_buffer_until_prompt_response() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "你好".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "！我是 Kimi Code CLI".to_string(),
            },
        );

        assert!(model.transcript_plain_items().is_empty());
        assert!(model.current_acp_activity_render_result().has_content);

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: String::new(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec!["你好！我是 Kimi Code CLI".to_string()]
        );
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn acp_permission_request_flushes_buffered_agent_text() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "需要先确认".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PermissionRequested {
                agent_id: "Kimi Code CLI".to_string(),
                request: crate::runtime::acp::AcpPermissionRequest {
                    request_id: "permission-1".to_string(),
                    title: Some("Write file".to_string()),
                    options: Vec::new(),
                },
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec!["需要先确认".to_string()]
        );
        assert!(model.current_status_notice_text().is_empty());
        assert!(model.tool_approval_panel_active());

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "确认后继续".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: String::new(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec!["需要先确认".to_string(), "确认后继续".to_string()]
        );
    }

    #[test]
    fn acp_agent_chunks_update_token_activity_without_flushing_transcript() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.transcript_mut().clear();
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "hello from acp".to_string(),
            },
        );

        let activity = model
            .current_acp_activity_render_result_at(
                std::time::Instant::now() + std::time::Duration::from_millis(120),
            )
            .plain_line;
        assert!(activity.contains("↓"));
        assert!(activity.contains("tokens"));
        assert!(model.transcript_plain_items().is_empty());
    }

    #[test]
    fn acp_prompt_started_keeps_submitted_activity_status_line() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                status_phrases: vec!["Submitted".to_string(), "Started".to_string()],
                status_phrase_order: StatusPhraseOrder::Cycle,
                ..ModelOptions::default()
            },
        );
        model.set_window(80, 6);
        model.show_acp_activity("Kimi Code CLI");
        let before = model.current_acp_activity_render_result().plain_line;
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );

        let after = model.current_acp_activity_render_result().plain_line;
        assert!(before.contains("Submitted (0s"));
        assert_eq!(after, before);
    }

    #[test]
    fn acp_prompt_response_updates_last_request_metrics() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                status_line_items: vec![StatusLineItem::Throughput, StatusLineItem::Latency],
                ..ModelOptions::default()
            },
        );
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "hello".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: String::new(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        let parts = model.current_status_line_parts();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].ends_with("tps"));
        assert!(parts[1].ends_with('s'));
    }

    #[test]
    fn acp_thought_chunks_append_reasoning_and_toggle_activity() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                reasoning_display_mode: ReasoningDisplayMode::Expanded,
                ..ModelOptions::default()
            },
        );
        model.set_window(80, 8);
        model.transcript_mut().clear();
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentThoughtChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "先分析".to_string(),
            },
        );

        assert!(
            model
                .current_acp_activity_render_result()
                .plain_line
                .contains("thinking")
        );

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "结论".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: String::new(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Hide reasoning · thoughts <1s]\n先分析".to_string(),
                "结论".to_string()
            ]
        );
        assert_eq!(
            model.transcript_mut().source_messages(),
            vec![(Sender::Assistant, "结论".to_string())]
        );
    }

    #[test]
    fn acp_thought_chunks_update_token_activity_like_native_reasoning() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 8);
        model.transcript_mut().clear();
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentThoughtChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "先分析这个问题的约束和实现路径。".to_string(),
            },
        );

        let activity = model
            .current_acp_activity_render_result_at(
                std::time::Instant::now() + std::time::Duration::from_millis(120),
            )
            .plain_line;
        assert!(activity.contains("thinking"));
        assert!(activity.contains("↓"));
        assert!(activity.contains("tokens"));
    }

    #[test]
    fn acp_model_config_changed_updates_current_model_status_line_and_models_panel() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                status_line_items: vec![StatusLineItem::CurrentModel],
                ..ModelOptions::default()
            },
        );
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::ModelConfigChanged {
                agent_id: "Kimi Code CLI".to_string(),
                config: crate::runtime::acp::AcpModelConfig {
                    config_id: "model".to_string(),
                    current_value: "kimi-k2".to_string(),
                    current_name: "Kimi K2".to_string(),
                    options: vec![crate::runtime::acp::AcpModelOption {
                        value: "kimi-k2".to_string(),
                        name: "Kimi K2".to_string(),
                    }],
                },
            },
        );

        assert_eq!(
            model.current_status_line_parts(),
            vec!["Kimi K2".to_string()]
        );
        let provider = model
            .model_catalog
            .enabled_provider_by_id("acp:Kimi Code CLI")
            .expect("ACP provider should replace model catalog");
        assert_eq!(provider.models[0].id, "kimi-k2");
        assert_eq!(
            model.selected_model,
            Some(ModelSelection::new("acp:Kimi Code CLI", "kimi-k2"))
        );
    }

    #[test]
    fn clear_runtime_discards_stale_native_chat_event() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");
        let mut acp_runtime = AcpRuntimeState::default();
        let mut native_chat_runtime = NativeChatRuntimeState::default();
        let (sender, receiver) = mpsc::channel();
        native_chat_runtime.receiver = Some(receiver);

        sender
            .send(NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "stale response".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
            .expect("stale native event should still be produced by worker");
        model.reset_to_initial_tui_state();
        reset_runtime_session_after_clear(
            &mut acp_runtime,
            &mut native_chat_runtime,
            &mut ModelProviderRefreshRuntimeState::default(),
        );
        drain_native_chat_runtime_events(&mut model, &mut native_chat_runtime);

        assert!(
            model
                .transcript_plain_items()
                .iter()
                .all(|item| !item.contains("stale response"))
        );
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn clear_runtime_discards_stale_acp_prompt_output_without_exiting_acp_mode() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        let mut acp_runtime = AcpRuntimeState::default();
        let mut native_chat_runtime = NativeChatRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "old partial".to_string(),
            },
        );

        model.reset_to_initial_tui_state();
        reset_runtime_session_after_clear(
            &mut acp_runtime,
            &mut native_chat_runtime,
            &mut ModelProviderRefreshRuntimeState::default(),
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: " stale response".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: " tail".to_string(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
        assert!(
            model
                .transcript_plain_items()
                .iter()
                .all(|item| !item.contains("old partial") && !item.contains("stale response"))
        );
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn clear_runtime_discards_stale_acp_prompt_start_activity() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        let mut acp_runtime = AcpRuntimeState::default();
        let mut native_chat_runtime = NativeChatRuntimeState::default();

        acp_runtime.mark_prompt_submitted();
        model.reset_to_initial_tui_state();
        reset_runtime_session_after_clear(
            &mut acp_runtime,
            &mut native_chat_runtime,
            &mut ModelProviderRefreshRuntimeState::default(),
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );

        assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn clear_runtime_discards_stale_acp_permission_request() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        let mut acp_runtime = AcpRuntimeState::default();
        let mut native_chat_runtime = NativeChatRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "旧请求需要权限".to_string(),
            },
        );

        model.reset_to_initial_tui_state();
        reset_runtime_session_after_clear(
            &mut acp_runtime,
            &mut native_chat_runtime,
            &mut ModelProviderRefreshRuntimeState::default(),
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PermissionRequested {
                agent_id: "Kimi Code CLI".to_string(),
                request: crate::runtime::acp::AcpPermissionRequest {
                    request_id: "stale-permission".to_string(),
                    title: Some("旧请求写文件".to_string()),
                    options: Vec::new(),
                },
            },
        );

        assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
        assert!(model.current_status_notice_text().is_empty());
        assert!(
            model
                .transcript_plain_items()
                .iter()
                .all(|item| !item.contains("旧请求"))
        );
    }

    #[test]
    fn acp_permission_cancel_reject_fallback_uses_reject_always() {
        use crate::runtime::acp::{
            AcpPermissionOption, AcpPermissionOptionKind, AcpPermissionRequest,
        };

        let options = acp_permission_option_ids(&AcpPermissionRequest {
            request_id: "permission-session-only".to_string(),
            title: Some("Run command".to_string()),
            options: vec![AcpPermissionOption {
                option_id: "reject-always".to_string(),
                name: "Reject in session".to_string(),
                kind: AcpPermissionOptionKind::RejectAlways,
            }],
        });

        assert_eq!(
            options.reject_for_cancel(),
            Some("reject-always".to_string())
        );
    }

    #[test]
    fn native_chat_completion_appends_assistant_message_after_request_finishes() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "你好，我是本地模型".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec!["你好，我是本地模型".to_string()]
        );
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn native_chat_completion_updates_last_request_metrics() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                status_line_items: vec![StatusLineItem::Throughput, StatusLineItem::Latency],
                ..ModelOptions::default()
            },
        );

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: Some(ChatPerformanceMetrics {
                    latency: std::time::Duration::from_millis(250),
                    output_tokens: 80,
                    duration: std::time::Duration::from_secs(2),
                }),
            },
        );

        assert_eq!(
            model.current_status_line_parts(),
            vec!["40tps".to_string(), "0.25s".to_string()]
        );
    }

    #[test]
    fn native_chat_completion_collapses_reasoning_by_default() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                ..ModelOptions::default()
            },
        );
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Show reasoning · thoughts 3s]".to_string(),
                "结论".to_string()
            ]
        );
        assert_eq!(
            model.transcript_mut().source_messages(),
            vec![(Sender::Assistant, "结论".to_string())]
        );
    }

    #[test]
    fn native_chat_completion_keeps_reasoning_body_gap_to_one_line() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                reasoning_display_mode: ReasoningDisplayMode::Expanded,
                ..ModelOptions::default()
            },
        );
        model.transcript_mut().clear();
        model.transcript_mut().set_width(40);
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        let render = model.transcript_mut().render();

        assert_eq!(
            render.all_plain_lines(),
            vec!["[Hide reasoning · thoughts 3s]", "先分析", "", "结论"]
        );
    }

    #[test]
    fn native_chat_reasoning_header_click_toggles_visibility_without_changing_source_messages() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                ..ModelOptions::default()
            },
        );
        model.set_palette(crate::frontend::tui::theme::default_palette(), true);
        model.set_window(40, 8);
        model.transcript_mut().clear();

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Show reasoning · thoughts 3s]".to_string(),
                "结论".to_string()
            ]
        );

        assert!(
            model
                .update(AppEvent::MouseDown {
                    button: MouseButton::Left,
                    column: 2,
                    row: 0,
                })
                .is_none()
        );
        assert!(
            model
                .update(AppEvent::MouseUp {
                    button: MouseButton::Left,
                    column: 2,
                    row: 0,
                })
                .is_none()
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Hide reasoning · thoughts 3s]\n先分析".to_string(),
                "结论".to_string()
            ]
        );
        assert_eq!(
            model.transcript_mut().source_messages(),
            vec![(Sender::Assistant, "结论".to_string())]
        );
    }

    #[test]
    fn native_chat_reasoning_header_drag_does_not_toggle() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                ..ModelOptions::default()
            },
        );
        model.set_palette(crate::frontend::tui::theme::default_palette(), true);
        model.set_window(40, 8);
        model.transcript_mut().clear();

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        assert!(
            model
                .update(AppEvent::MouseDown {
                    button: MouseButton::Left,
                    column: 2,
                    row: 0,
                })
                .is_none()
        );
        assert!(
            model
                .update(AppEvent::MouseDrag {
                    button: MouseButton::Left,
                    column: 8,
                    row: 0,
                })
                .is_none()
        );
        assert!(
            model
                .update(AppEvent::MouseUp {
                    button: MouseButton::Left,
                    column: 8,
                    row: 0,
                })
                .is_none()
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Show reasoning · thoughts 3s]".to_string(),
                "结论".to_string()
            ]
        );
    }

    #[test]
    fn native_chat_reasoning_header_click_outside_label_does_not_toggle() {
        let mut model = Model::new_with_options(
            HeroOptions::default(),
            ModelOptions {
                show_reasoning_content: true,
                ..ModelOptions::default()
            },
        );
        model.set_palette(crate::frontend::tui::theme::default_palette(), true);
        model.set_window(40, 8);
        model.transcript_mut().clear();

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        assert!(
            model
                .update(AppEvent::MouseDown {
                    button: MouseButton::Left,
                    column: 38,
                    row: 0,
                })
                .is_none()
        );
        assert!(
            model
                .update(AppEvent::MouseUp {
                    button: MouseButton::Left,
                    column: 38,
                    row: 0,
                })
                .is_none()
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec![
                "[Show reasoning · thoughts 3s]".to_string(),
                "结论".to_string()
            ]
        );
    }

    #[test]
    fn native_chat_completion_hides_reasoning_when_configured_off() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "结论".to_string(),
                    reasoning_content: Some("先分析".to_string()),
                    reasoning_duration: Some(std::time::Duration::from_secs(3)),
                },
                metrics: None,
            },
        );

        assert_eq!(model.transcript_plain_items(), vec!["结论".to_string()]);
    }

    #[test]
    fn native_chat_thinking_event_toggles_activity_segment() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(80, 6);
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(&mut model, NativeChatEvent::Thinking { is_thinking: true });

        assert!(
            model
                .current_acp_activity_render_result()
                .plain_line
                .contains("thinking")
        );

        apply_native_chat_event(&mut model, NativeChatEvent::Thinking { is_thinking: false });

        assert!(
            !model
                .current_acp_activity_render_result()
                .plain_line
                .contains("thinking")
        );
    }

    #[test]
    fn native_chat_failure_appends_system_message_in_transcript() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Failed {
                message: "request /v1/chat/completions: connection refused".to_string(),
            },
        );

        assert_eq!(
            model.transcript_plain_items(),
            vec!["■ Chat failed: request /v1/chat/completions: connection refused".to_string()]
        );
        assert!(model.current_status_notice_text().is_empty());
        assert!(!model.current_acp_activity_render_result().has_content);
    }

    #[test]
    fn native_chat_retry_event_shows_reconnecting_activity() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            },
        );

        let activity = model.current_acp_activity_render_result().plain_line;
        assert!(activity.contains("Reconnecting... 1/3"));
        assert!(model.transcript_plain_items().is_empty());
    }

    #[test]
    fn runtime_request_policy_uses_configured_delay_and_timeout() {
        let policy = RuntimeRequestPolicy::new(5, vec![1, 3, 5, 5, 5], 120);

        assert_eq!(policy.attempts(), 5);
        assert_eq!(policy.delay_for_retry(1), Duration::from_secs(1));
        assert_eq!(policy.delay_for_retry(2), Duration::from_secs(3));
        assert_eq!(policy.delay_for_retry(3), Duration::from_secs(5));
        assert_eq!(policy.delay_for_retry(5), Duration::from_secs(5));
        assert_eq!(policy.timeout(), Duration::from_secs(120));
    }

    #[test]
    fn native_chat_token_estimate_updates_activity_without_finishing_request() {
        let mut model = Model::new(HeroOptions::default());
        model.set_window(70, 6);
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_native_chat_event(
            &mut model,
            NativeChatEvent::OutputTokenEstimate { total_tokens: 32 },
        );

        let activity = model
            .current_acp_activity_render_result_at(
                std::time::Instant::now() + std::time::Duration::from_millis(120),
            )
            .plain_line;
        assert!(activity.contains("↓ 32 tokens"));
        assert!(model.current_acp_activity_render_result().has_content);
        assert!(model.transcript_plain_items().is_empty());
    }

    #[test]
    fn native_chat_runtime_keeps_receiver_after_retry_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeChatRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::default()),
        };

        sender
            .send(NativeChatEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
            .expect("retry event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeChatEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
        );
        assert!(runtime.is_running());

        sender
            .send(NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
            .expect("finish event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeChatEvent::Finished {
                response: NativeChatResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
        );
        assert!(!runtime.is_running());
    }

    #[test]
    fn native_chat_runtime_keeps_receiver_after_token_estimate_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeChatRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::default()),
        };

        sender
            .send(NativeChatEvent::OutputTokenEstimate { total_tokens: 12 })
            .expect("token estimate event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeChatEvent::OutputTokenEstimate { total_tokens: 12 })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn interrupt_native_chat_clears_runtime_and_appends_system_message() {
        let (_sender, receiver) = mpsc::channel();
        let mut runtime = NativeChatRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::default()),
        };
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.show_acp_activity("qwen3");

        apply_effect_if_needed_for_test(
            &mut model,
            &mut runtime,
            Some(AppEffect::InterruptCurrentTurn),
        );

        assert!(!runtime.is_running());
        assert!(!model.current_acp_activity_render_result().has_content);
        assert_eq!(
            model.transcript_plain_items(),
            vec!["■ Chat interrupted".to_string()]
        );
    }

    #[test]
    fn interrupt_acp_prompt_discards_stale_output_and_keeps_session_selected() {
        let mut model = Model::new(HeroOptions::default());
        model.transcript_mut().clear();
        model.selected_acp_agent = Some("Kimi Code CLI".to_string());
        let mut acp_runtime = AcpRuntimeState::default();

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptStarted {
                agent_id: "Kimi Code CLI".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: "partial before interrupt".to_string(),
            },
        );

        run_interrupt_acp_prompt_effect(&mut model, &mut acp_runtime);

        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::AgentMessageChunk {
                agent_id: "Kimi Code CLI".to_string(),
                content: " stale response".to_string(),
            },
        );
        apply_acp_session_event(
            &mut model,
            &mut acp_runtime,
            AcpSessionEvent::PromptResponse {
                agent_id: "Kimi Code CLI".to_string(),
                content: " tail".to_string(),
                stop_reason: "EndTurn".to_string(),
            },
        );

        assert_eq!(model.selected_acp_agent(), Some("Kimi Code CLI"));
        assert!(!model.current_acp_activity_render_result().has_content);
        assert_eq!(
            model.transcript_plain_items(),
            vec!["■ Chat interrupted".to_string()]
        );
    }

    #[test]
    fn ready_input_batch_coalesces_wheel_burst_before_key() {
        let events = (0..128)
            .map(|_| {
                Event::Mouse(MouseEvent {
                    kind: MouseEventKind::ScrollUp,
                    column: 0,
                    row: 0,
                    modifiers: KeyModifiers::empty(),
                })
            })
            .chain(std::iter::once(Event::Key(KeyEvent::from(KeyCode::Char(
                'x',
            )))))
            .collect::<Vec<_>>();

        let actions = coalesced_input_actions(events);

        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[0],
            TerminalInputAction::App(AppEvent::MouseWheel {
                delta_lines: -128 * Model::document_mouse_wheel_delta(),
            })
        );
        assert_eq!(
            actions[1],
            TerminalInputAction::App(AppEvent::Key(KeyEvent::from(KeyCode::Char('x'))))
        );
    }

    fn apply_effect_if_needed_for_test(
        model: &mut Model,
        native_chat_runtime: &mut NativeChatRuntimeState,
        effect: Option<AppEffect>,
    ) {
        if let Some(AppEffect::InterruptCurrentTurn) = effect {
            run_interrupt_current_turn_effect(
                model,
                &mut AcpRuntimeState::default(),
                native_chat_runtime,
            );
        }
    }
}
