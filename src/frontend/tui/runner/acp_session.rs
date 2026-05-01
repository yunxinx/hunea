use std::time::{Duration, Instant};

use color_eyre::eyre::Result;

use crate::frontend::tui::{Model, RequestMetrics, runtime::RuntimeEventApply};
use crate::runtime::acp::{
    AcpAgentIdentity, AcpPrompt, AcpSessionCommand, AcpSessionEvent, AcpSessionWorker,
};
use crate::runtime::session::{RuntimeEvent, RuntimeTarget};
use crate::runtime::token_count::StreamingTokenProgress;

use super::RuntimeOptions;

#[derive(Default)]
pub(super) struct AcpRuntimeState {
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
    pub(super) fn should_poll_events(&self) -> bool {
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

    pub(super) fn mark_prompt_submitted(&mut self) {
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

    pub(super) fn reset_after_clear(&mut self) {
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

    fn send_prompt(&self, agent_id: &str, prompt: AcpPrompt) -> Result<(), String> {
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

pub(super) fn drain_acp_runtime_events(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
) -> bool {
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

pub(super) fn apply_acp_session_event(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    event: AcpSessionEvent,
) {
    match event {
        AcpSessionEvent::Started {
            agent_id, outcome, ..
        } => {
            model.apply_acp_agent_identity(
                agent_id,
                AcpAgentIdentity::from_initialize_outcome(&outcome),
            );
            model.show_transient_status_notice(&format!(
                "ACP session ready: {}",
                crate::runtime::acp::agent_display_name(&outcome)
            ));
        }
        AcpSessionEvent::StartFailed { message, .. } => {
            model.show_transient_status_notice(&format!("ACP start failed: {message}"));
        }
        AcpSessionEvent::SystemMessage { message, .. } => {
            model.append_system_message_from_runtime(message);
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
            if model.stream_activity.is_none() {
                model.show_stream_activity(agent_id);
            }
        }
        AcpSessionEvent::AgentThoughtChunk { content, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            acp_runtime.push_reasoning_chunk(&content);
            model.set_stream_activity_thinking(true);
            if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
            }
        }
        AcpSessionEvent::AgentMessageChunk { content, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            model.set_stream_activity_thinking(false);
            acp_runtime.push_response_chunk(&content);
            if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
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
                model.clear_stream_activity();
                return;
            }
            if !content.is_empty() {
                acp_runtime.push_response_chunk(&content);
                if let Some(total_tokens) = acp_runtime.observe_output_tokens(&content) {
                    model.set_stream_activity_output_tokens(total_tokens);
                }
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            let metrics = acp_runtime.request_metrics(Instant::now());
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            if let Some(metrics) = metrics {
                model.set_last_request_metrics(Some(metrics));
            }
            acp_runtime.mark_prompt_finished();
            model.clear_stream_activity();
            if stop_reason != "EndTurn" {
                model.show_transient_status_notice(&format!("ACP prompt finished: {stop_reason}"));
            }
        }
        AcpSessionEvent::PromptFailed { message, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                acp_runtime.mark_prompt_finished();
                model.clear_stream_activity();
                return;
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            acp_runtime.mark_prompt_finished();
            model.clear_stream_activity();
            model.show_transient_status_notice(&format!("ACP prompt failed: {message}"));
        }
        AcpSessionEvent::PromptInterrupted { .. } => {
            acp_runtime.mark_prompt_finished();
            model.clear_stream_activity();
        }
        AcpSessionEvent::PermissionRequested { agent_id, request } => {
            if acp_runtime.should_discard_prompt_output() {
                let _ = acp_runtime.respond_permission(
                    &request.request_id,
                    acp_reject_option_id_for_cancel(&request),
                );
                return;
            }
            if let Some(total_tokens) = acp_runtime.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_runtime);
            model.apply_runtime_event(RuntimeEvent::PermissionRequested {
                target: RuntimeTarget::acp_agent(agent_id),
                request: request.into(),
            });
        }
        AcpSessionEvent::PermissionRequestCancelled { agent_id } => {
            if acp_runtime.should_discard_prompt_output() {
                return;
            }
            model.apply_runtime_event(RuntimeEvent::PermissionCancelled {
                target: RuntimeTarget::acp_agent(agent_id),
                request_id: None,
            });
        }
        AcpSessionEvent::Stopped { message, .. } => {
            if acp_runtime.should_discard_prompt_output() {
                acp_runtime.mark_prompt_finished();
                model.clear_stream_activity();
                return;
            }
            flush_acp_response_buffer(model, acp_runtime);
            model.clear_stream_activity();
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

pub(super) fn run_start_acp_session_effect(
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

pub(super) fn run_send_acp_prompt_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
    agent_id: &str,
    prompt: AcpPrompt,
) {
    if let Err(message) = acp_runtime.send_prompt(agent_id, prompt) {
        model.clear_stream_activity();
        model.show_transient_status_notice(&message);
    } else {
        acp_runtime.mark_prompt_submitted();
    }
}

pub(super) fn run_respond_acp_permission_effect(
    model: &mut Model,
    acp_runtime: &AcpRuntimeState,
    request_id: &str,
    option_id: Option<String>,
) {
    if let Err(message) = acp_runtime.respond_permission(request_id, option_id) {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn run_set_acp_model_effect(
    model: &mut Model,
    acp_runtime: &AcpRuntimeState,
    config_id: String,
    value: String,
) {
    if let Err(message) = acp_runtime.set_config_option(config_id, value) {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn run_interrupt_acp_prompt_effect(
    model: &mut Model,
    acp_runtime: &mut AcpRuntimeState,
) {
    if !acp_runtime.interrupt_prompt() {
        return;
    }

    if let Some(pending) = model.pending_acp_permission.take() {
        let _ = acp_runtime.respond_permission(&pending.request_id, None);
        model.close_tool_approval_panel();
    }
    model.clear_stream_activity();
    model.append_system_message_from_runtime("Chat interrupted");
}

pub(super) fn acp_reject_option_id_for_cancel(
    request: &crate::runtime::acp::AcpPermissionRequest,
) -> Option<String> {
    use crate::runtime::acp::AcpPermissionOptionKind;

    request
        .options
        .iter()
        .find(|option| option.kind == AcpPermissionOptionKind::RejectOnce)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == AcpPermissionOptionKind::RejectAlways)
        })
        .map(|option| option.option_id.clone())
}
