use std::{
    io,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

use crate::runtime::session::{
    AcpInitializeOutcome, AcpSessionCatalog, AcpSessionCommand, AcpSessionEvent, AcpSessionWorker,
};
use crate::runtime::{models, models::ModelSelection};
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
    AppEffect, AppEvent, HeroOptions, Model, ModelOptions, STARTUP_PROBE_TIMEOUT, StyleMode, theme,
};

/// `RuntimeOptions` 表示 TUI runner 可执行的外部 runtime 能力。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    pub acp_sessions: AcpSessionCatalog,
    pub model_config_path: Option<PathBuf>,
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

    loop {
        drain_acp_runtime_events(&mut model, &mut acp_runtime);
        terminal.draw(|frame| model.render(frame))?;

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
                effect,
            )?;
            continue;
        }

        if let Some(timeout_event) = model.timeout_event(now) {
            let effect = model.update(timeout_event);
            apply_effect_if_needed(
                &mut terminal,
                &mut model,
                &runtime_options,
                &mut acp_runtime,
                effect,
            )?;
            continue;
        }

        let wait_duration = next_wait_duration(&model, startup_deadline, now);

        if !event::poll(wait_duration)? {
            if !model.has_palette() {
                let effect = model.update(AppEvent::StartupReadyTimeout);
                apply_effect_if_needed(
                    &mut terminal,
                    &mut model,
                    &runtime_options,
                    &mut acp_runtime,
                    effect,
                )?;
            } else if let Some(timeout_event) = model.timeout_event(Instant::now()) {
                let effect = model.update(timeout_event);
                apply_effect_if_needed(
                    &mut terminal,
                    &mut model,
                    &runtime_options,
                    &mut acp_runtime,
                    effect,
                )?;
            }
            continue;
        }

        let terminal_events = read_ready_terminal_events(event::read()?)?;
        for action in coalesced_input_actions(terminal_events) {
            match action {
                TerminalInputAction::App(app_event) => {
                    let effect = model.update(app_event);
                    apply_effect_if_needed(
                        &mut terminal,
                        &mut model,
                        &runtime_options,
                        &mut acp_runtime,
                        effect,
                    )?;
                }
                TerminalInputAction::CancelExitConfirmation => model.cancel_exit_confirmation(),
            }

            if model.is_quitting() {
                break;
            }
        }
    }

    Ok(model)
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

fn next_wait_duration(model: &Model, startup_deadline: Instant, now: Instant) -> Duration {
    let mut next_deadline = if model.has_palette() {
        None
    } else {
        Some(startup_deadline)
    };

    if let Some(model_deadline) = model.next_timeout_deadline() {
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(model_deadline),
            None => model_deadline,
        });
    }

    if let Some(activity_interval) = model.acp_activity_frame_interval() {
        let activity_deadline = now + activity_interval;
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(activity_deadline),
            None => activity_deadline,
        });
    }

    next_deadline
        .map(|deadline| deadline.saturating_duration_since(now))
        .unwrap_or_else(|| Duration::from_millis(250))
}

fn apply_effect_if_needed(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: &mut Model,
    runtime_options: &RuntimeOptions,
    acp_runtime: &mut AcpRuntimeState,
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
        AppEffect::PersistSelectedModel { selection } => {
            run_persist_selected_model_effect(model, runtime_options, &selection);
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

#[derive(Default)]
struct AcpRuntimeState {
    worker: Option<AcpSessionWorker>,
    response_buffer: String,
}

impl AcpRuntimeState {
    fn start(&mut self, command: AcpSessionCommand) {
        self.response_buffer.clear();
        self.worker = Some(AcpSessionWorker::start(command));
    }

    fn reset_response_buffer(&mut self) {
        self.response_buffer.clear();
    }

    fn push_response_chunk(&mut self, content: &str) {
        self.response_buffer.push_str(content);
    }

    fn take_response_buffer(&mut self) -> Option<String> {
        if self.response_buffer.is_empty() {
            return None;
        }

        Some(std::mem::take(&mut self.response_buffer))
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
}

fn drain_acp_runtime_events(model: &mut Model, acp_runtime: &mut AcpRuntimeState) {
    let Some(worker) = acp_runtime.worker.as_ref() else {
        return;
    };

    let mut events = Vec::new();
    while let Some(event) = worker.try_recv_event() {
        events.push(event);
    }

    for event in events {
        apply_acp_session_event(model, acp_runtime, event);
    }
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
            model.show_acp_activity(agent_id);
        }
        AcpSessionEvent::AgentMessageChunk { content, .. } => {
            acp_runtime.push_response_chunk(&content);
        }
        AcpSessionEvent::PromptResponse {
            content,
            stop_reason,
            ..
        } => {
            if !content.is_empty() {
                acp_runtime.push_response_chunk(&content);
            }
            flush_acp_response_buffer(model, acp_runtime);
            model.clear_acp_activity();
            if stop_reason != "EndTurn" {
                model.show_transient_status_notice(&format!("ACP prompt finished: {stop_reason}"));
            }
        }
        AcpSessionEvent::PromptFailed { message, .. } => {
            flush_acp_response_buffer(model, acp_runtime);
            model.clear_acp_activity();
            model.show_transient_status_notice(&format!("ACP prompt failed: {message}"));
        }
        AcpSessionEvent::PermissionRequested { request, .. } => {
            flush_acp_response_buffer(model, acp_runtime);
            let (allow_option_id, reject_option_id) = acp_permission_option_ids(&request);
            model.update(AppEvent::AcpPermissionRequested {
                request_id: request.request_id,
                title: request.title,
                allow_option_id,
                reject_option_id,
            });
        }
        AcpSessionEvent::PermissionRequestCancelled { .. } => {
            model.show_transient_status_notice("ACP permission request cancelled");
        }
        AcpSessionEvent::Stopped { message, .. } => {
            flush_acp_response_buffer(model, acp_runtime);
            model.clear_acp_activity();
            if let Some(message) = message {
                model.show_transient_status_notice(&format!("ACP session stopped: {message}"));
            }
        }
    }
}

fn flush_acp_response_buffer(model: &mut Model, acp_runtime: &mut AcpRuntimeState) {
    if let Some(content) = acp_runtime.take_response_buffer() {
        model.append_assistant_message_from_runtime(content);
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
    model.show_transient_status_notice(&format!("Starting ACP agent: {agent_id}"));
    Ok(())
}

fn run_send_acp_prompt_effect(
    model: &mut Model,
    acp_runtime: &AcpRuntimeState,
    agent_id: &str,
    prompt: String,
) {
    if let Err(message) = acp_runtime.send_prompt(agent_id, prompt) {
        model.show_transient_status_notice(&message);
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

fn acp_permission_option_ids(
    request: &crate::runtime::session::AcpPermissionRequest,
) -> (Option<String>, Option<String>) {
    use crate::runtime::session::AcpPermissionOptionKind;

    let allow = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::AllowOnce)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == AcpPermissionOptionKind::AllowAlways)
        })
        .or_else(|| {
            request.options.iter().find(|option| {
                matches!(
                    option.kind,
                    AcpPermissionOptionKind::AllowOnce | AcpPermissionOptionKind::AllowAlways
                )
            })
        })
        .map(|option| option.option_id.clone());

    let reject = request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::RejectOnce)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == AcpPermissionOptionKind::RejectAlways)
        })
        .map(|option| option.option_id.clone());

    (allow, reject)
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
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

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
                request: crate::runtime::session::AcpPermissionRequest {
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
        assert!(model.current_status_notice_text().contains("Write file"));

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
}
