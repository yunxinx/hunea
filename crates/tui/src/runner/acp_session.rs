use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use crate::{
    AcpPromptSubmission, Model, RequestMetrics, acp::AcpPermissionPanelRequest,
    acp_tool_preview::ToolApprovalPreview, runtime::RuntimeEventApply,
};
use mo_core::acp::{
    AcpAgentIdentity, AcpPermissionOptionKind, AcpPermissionRequest, AcpSessionEvent,
    AcpTerminalSnapshot, AcpToolCall, AcpToolCallContent, AcpToolCallStatus, AcpToolCallUpdate,
    AcpToolKind,
};
use mo_core::session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, RuntimeTarget};
use mo_core::token_count::StreamingTokenProgress;

#[cfg(test)]
use super::NoopRuntimeCoordinator;
use super::RuntimeCoordinator;

/// `AcpSessionUiState` 保存 ACP 流式输出映射到 TUI 所需的临时状态。
#[derive(Default)]
pub(super) struct AcpSessionUiState {
    response_buffer: String,
    reasoning_buffer: String,
    reasoning_started_at: Option<Instant>,
    pending_rejected_permission_notice_suppression: bool,
    prompt_in_flight: bool,
    discard_in_flight_prompt: Option<PromptDiscardReason>,
    token_progress: Option<StreamingTokenProgress>,
    prompt_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    tool_call_items: HashMap<String, usize>,
    tool_call_terminal_ids: HashMap<String, HashSet<String>>,
    terminal_active_states: HashMap<String, bool>,
    tool_call_token_text: HashMap<String, String>,
    rejected_permission_tool_calls: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptDiscardReason {
    Cancelled,
    Stale,
}

impl AcpSessionUiState {
    fn reset_for_new_session(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.prompt_in_flight = false;
        self.discard_in_flight_prompt = None;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.terminal_active_states.clear();
        self.tool_call_token_text.clear();
        self.rejected_permission_tool_calls.clear();
    }

    fn reset_response_buffer(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
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

        if self.pending_rejected_permission_notice_suppression {
            if is_agent_facing_permission_rejection_notice(&self.response_buffer) {
                self.response_buffer.clear();
                self.pending_rejected_permission_notice_suppression = false;
                return None;
            }

            if is_agent_facing_permission_rejection_notice_prefix(&self.response_buffer) {
                return None;
            }

            self.pending_rejected_permission_notice_suppression = false;
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
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }

    fn start_token_progress(&mut self, model_id: impl Into<String>) {
        self.token_progress = Some(StreamingTokenProgress::new(model_id));
    }

    fn observe_output_tokens(&mut self, content: &str) -> Option<usize> {
        if !content.is_empty() {
            self.first_token_at.get_or_insert_with(Instant::now);
        }
        self.token_progress
            .as_mut()
            .and_then(|progress| progress.observe_delta(content, Instant::now()))
    }

    fn observe_tool_call_tokens(
        &mut self,
        tool_call_id: &str,
        projected_text: Option<String>,
    ) -> Option<usize> {
        let projected_text = projected_text?;
        if projected_text.is_empty() {
            return None;
        }

        let previous = self
            .tool_call_token_text
            .entry(tool_call_id.to_string())
            .or_default();
        let delta = if projected_text.starts_with(previous.as_str()) {
            projected_text[previous.len()..].to_string()
        } else {
            projected_text.clone()
        };
        *previous = projected_text;

        self.observe_output_tokens(&delta)
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
        self.discard_in_flight_prompt = None;
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }

    fn should_discard_prompt_output(&self) -> bool {
        self.discard_in_flight_prompt.is_some()
    }

    pub(super) fn permission_option_id_for_discarded_prompt(
        &self,
        request: &AcpPermissionRequest,
    ) -> Option<String> {
        match self.discard_in_flight_prompt {
            Some(PromptDiscardReason::Cancelled) => None,
            Some(PromptDiscardReason::Stale) | None => {
                acp_reject_option_id_for_stale_discard(request)
            }
        }
    }

    fn suppress_rejected_permission_notice_for_tool_call(&mut self, tool_call_id: Option<String>) {
        self.pending_rejected_permission_notice_suppression = true;
        if let Some(tool_call_id) = tool_call_id {
            self.rejected_permission_tool_calls.insert(tool_call_id);
        }
    }

    fn should_sanitize_rejected_permission_tool_update(&self, tool_call_id: &str) -> bool {
        self.pending_rejected_permission_notice_suppression
            || self.rejected_permission_tool_calls.contains(tool_call_id)
    }

    fn interrupt_prompt(&mut self) -> bool {
        if !self.prompt_in_flight {
            return false;
        }
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.discard_in_flight_prompt = Some(PromptDiscardReason::Cancelled);
        self.tool_call_token_text.clear();
        true
    }

    pub(super) fn reset_after_clear(&mut self) {
        self.response_buffer.clear();
        self.reasoning_buffer.clear();
        self.reasoning_started_at = None;
        self.pending_rejected_permission_notice_suppression = false;
        self.token_progress = None;
        self.prompt_started_at = None;
        self.first_token_at = None;
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
        self.rejected_permission_tool_calls.clear();
        if self.prompt_in_flight && self.discard_in_flight_prompt.is_none() {
            self.discard_in_flight_prompt = Some(PromptDiscardReason::Stale);
        }
    }

    fn track_tool_call(&mut self, tool_call_id: String, item_index: usize) {
        self.tool_call_items.insert(tool_call_id, item_index);
    }

    fn track_tool_call_terminal_content(
        &mut self,
        tool_call_id: &str,
        content: Option<&[AcpToolCallContent]>,
    ) {
        let Some(content) = content else {
            return;
        };
        let terminal_ids = content
            .iter()
            .filter_map(|content| match content {
                AcpToolCallContent::Terminal { terminal_id } => Some(terminal_id.clone()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        if terminal_ids.is_empty() {
            self.tool_call_terminal_ids.remove(tool_call_id);
        } else {
            self.tool_call_terminal_ids
                .insert(tool_call_id.to_string(), terminal_ids);
        }
    }

    fn observe_terminal_snapshot(&mut self, snapshot: &AcpTerminalSnapshot) {
        self.terminal_active_states.insert(
            snapshot.terminal_id.clone(),
            snapshot.exit_status.is_none() && !snapshot.released,
        );
    }

    fn tool_call_item_index(&self, tool_call_id: &str) -> Option<usize> {
        self.tool_call_items.get(tool_call_id).copied()
    }

    fn tracked_non_background_tool_call_indices(&self) -> Vec<usize> {
        self.tool_call_items
            .iter()
            .filter_map(|(tool_call_id, item_index)| {
                (!self.tool_call_has_running_or_pending_terminal(tool_call_id))
                    .then_some(*item_index)
            })
            .collect()
    }

    fn tool_call_has_running_or_pending_terminal(&self, tool_call_id: &str) -> bool {
        self.tool_call_terminal_ids
            .get(tool_call_id)
            .is_some_and(|terminal_ids| {
                terminal_ids.iter().any(|terminal_id| {
                    self.terminal_active_states
                        .get(terminal_id)
                        .copied()
                        .unwrap_or(true)
                })
            })
    }

    fn clear_tool_call_tracking(&mut self) {
        self.tool_call_items.clear();
        self.tool_call_terminal_ids.clear();
        self.tool_call_token_text.clear();
    }
}

const AGENT_FACING_PERMISSION_REJECTION_NOTICE: &str = concat!(
    "The tool call is rejected by the user. ",
    "Stop what you are doing and wait for the user to tell you how to proceed."
);

fn normalized_agent_text(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_agent_facing_permission_rejection_notice(content: &str) -> bool {
    let normalized = normalized_agent_text(content);
    normalized == AGENT_FACING_PERMISSION_REJECTION_NOTICE
        || (normalized.starts_with("The tool call is rejected by the user.")
            && normalized.contains("Stop what you are doing")
            && normalized.contains("wait for the user to tell you how to proceed"))
}

fn is_agent_facing_permission_rejection_notice_prefix(content: &str) -> bool {
    let normalized = normalized_agent_text(content);
    !normalized.is_empty() && AGENT_FACING_PERMISSION_REJECTION_NOTICE.starts_with(&normalized)
}

fn sanitize_rejected_permission_tool_call_update(
    update: &mut AcpToolCallUpdate,
    should_sanitize: bool,
) {
    if !should_sanitize {
        return;
    }

    if let Some(content) = update.content.take() {
        update.content = Some(
            content
                .into_iter()
                .filter(|content| !is_agent_facing_permission_rejection_tool_content(content))
                .collect(),
        );
    }

    if update
        .raw_input
        .as_ref()
        .is_some_and(is_agent_facing_permission_rejection_raw_value)
    {
        update.raw_input = None;
    }
    if update
        .raw_output
        .as_ref()
        .is_some_and(is_agent_facing_permission_rejection_raw_value)
    {
        update.raw_output = None;
    }
}

fn is_agent_facing_permission_rejection_tool_content(content: &AcpToolCallContent) -> bool {
    matches!(content, AcpToolCallContent::Text(text) if is_agent_facing_permission_rejection_notice(text))
}

fn is_agent_facing_permission_rejection_raw_value(
    raw_value: &mo_core::acp::AcpToolCallRawValue,
) -> bool {
    raw_value
        .display_text()
        .is_some_and(|text| is_agent_facing_permission_rejection_notice(&text))
}

#[cfg(test)]
pub(super) fn apply_acp_session_event(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    event: AcpSessionEvent,
) {
    let mut runtime_coordinator = NoopRuntimeCoordinator;
    apply_acp_session_event_with_coordinator(model, acp_ui_state, &mut runtime_coordinator, event);
}

pub(super) fn apply_acp_session_event_with_coordinator(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
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
                mo_core::acp::agent_display_name(&outcome)
            ));
        }
        AcpSessionEvent::StartFailed { message, .. } => {
            model.show_transient_status_notice(&format!("ACP start failed: {message}"));
        }
        AcpSessionEvent::SystemMessage { message, .. } => {
            model.append_system_message_from_runtime(message);
        }
        AcpSessionEvent::PromptStarted { agent_id } => {
            acp_ui_state.reset_response_buffer();
            acp_ui_state.mark_prompt_started();
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            acp_ui_state.start_token_progress(
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
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            acp_ui_state.push_reasoning_chunk(&content);
            model.set_stream_activity_thinking(true);
            if let Some(total_tokens) = acp_ui_state.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
            }
        }
        AcpSessionEvent::AgentMessageChunk { content, .. } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            model.set_stream_activity_thinking(false);
            acp_ui_state.push_response_chunk(&content);
            if let Some(total_tokens) = acp_ui_state.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
            }
        }
        AcpSessionEvent::ToolCall { call, .. } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            flush_acp_response_buffer(model, acp_ui_state);
            let tool_call_id = call.tool_call_id.clone();
            if let Some(total_tokens) = acp_ui_state
                .observe_tool_call_tokens(&tool_call_id, acp_tool_call_token_text(&call))
            {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            upsert_acp_tool_call(model, acp_ui_state, call);
            model.set_stream_activity_thinking(false);
        }
        AcpSessionEvent::ToolCallUpdate { mut update, .. } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            flush_acp_response_buffer(model, acp_ui_state);
            let tool_call_id = update.tool_call_id.clone();
            sanitize_rejected_permission_tool_call_update(
                &mut update,
                acp_ui_state.should_sanitize_rejected_permission_tool_update(&tool_call_id),
            );
            if let Some(total_tokens) = acp_ui_state
                .observe_tool_call_tokens(&tool_call_id, acp_tool_call_update_token_text(&update))
            {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            upsert_acp_tool_call_update(model, acp_ui_state, update, None);
            model.set_stream_activity_thinking(false);
        }
        AcpSessionEvent::ModelConfigChanged { agent_id, config } => {
            model.apply_acp_model_config(&agent_id, config);
        }
        AcpSessionEvent::AvailableCommandsChanged { agent_id, commands } => {
            model.apply_acp_available_commands(agent_id, commands);
        }
        AcpSessionEvent::ConfigChangeSucceeded { agent_id } => {
            model.commit_pending_acp_model_change(&agent_id);
        }
        AcpSessionEvent::ConfigChangeFailed { agent_id, message } => {
            model.rollback_pending_acp_model_change(&agent_id);
            model.show_transient_status_notice(&format!("ACP config change failed: {message}"));
        }
        AcpSessionEvent::PromptResponse {
            content,
            stop_reason,
            ..
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                acp_ui_state.mark_prompt_finished();
                model.clear_stream_activity();
                return;
            }
            if !content.is_empty() {
                acp_ui_state.push_response_chunk(&content);
                if let Some(total_tokens) = acp_ui_state.observe_output_tokens(&content) {
                    model.set_stream_activity_output_tokens(total_tokens);
                }
            }
            if let Some(total_tokens) = acp_ui_state.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            let metrics = acp_ui_state.request_metrics(Instant::now());
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_ui_state);
            if let Some(metrics) = metrics {
                model.set_last_request_metrics(Some(metrics));
            }
            fail_tracked_acp_tool_calls(
                model,
                acp_ui_state,
                "Tool call ended without final status",
            );
            acp_ui_state.mark_prompt_finished();
            model.clear_stream_activity();
            if stop_reason != "EndTurn" {
                model.show_transient_status_notice(&format!("ACP prompt finished: {stop_reason}"));
            }
        }
        AcpSessionEvent::PromptFailed { message, .. } => {
            if acp_ui_state.should_discard_prompt_output() {
                acp_ui_state.mark_prompt_finished();
                model.clear_stream_activity();
                return;
            }
            if let Some(total_tokens) = acp_ui_state.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_ui_state);
            fail_tracked_acp_tool_calls(
                model,
                acp_ui_state,
                "Tool call ended because the ACP prompt failed",
            );
            acp_ui_state.mark_prompt_finished();
            model.clear_stream_activity();
            model.show_transient_status_notice(&format!("ACP prompt failed: {message}"));
        }
        AcpSessionEvent::PromptInterrupted { .. } => {
            fail_tracked_acp_tool_calls(model, acp_ui_state, "Interrupted");
            acp_ui_state.mark_prompt_finished();
            model.clear_stream_activity();
        }
        AcpSessionEvent::PermissionRequested { agent_id, request } => {
            if acp_ui_state.should_discard_prompt_output() {
                let _ = runtime_coordinator.dispatch_runtime_command(
                    RuntimeCommand::RespondPermission {
                        target: Some(RuntimeTarget::acp_agent(agent_id.clone())),
                        request_id: request.request_id.clone(),
                        option_id: acp_ui_state.permission_option_id_for_discarded_prompt(&request),
                    },
                );
                return;
            }
            if let Some(total_tokens) = acp_ui_state.flush_output_tokens() {
                model.set_stream_activity_output_tokens(total_tokens);
            }
            model.set_stream_activity_thinking(false);
            flush_acp_response_buffer(model, acp_ui_state);
            let transcript_tool_call = acp_permission_transcript_tool_call_update(&request);
            let permission_tool_call_item_index = {
                let tool_call_id = transcript_tool_call.tool_call_id.clone();
                if let Some(total_tokens) = acp_ui_state.observe_tool_call_tokens(
                    &tool_call_id,
                    acp_tool_call_update_token_text(&transcript_tool_call),
                ) {
                    model.set_stream_activity_output_tokens(total_tokens);
                }
                upsert_acp_tool_call_update(
                    model,
                    acp_ui_state,
                    transcript_tool_call.clone(),
                    request.title.clone(),
                )
            };
            let preview = ToolApprovalPreview::from_acp_tool_call_update(&transcript_tool_call);
            let suspended_tool_call_item_index =
                preview.as_ref().map(|_| permission_tool_call_item_index);
            model.set_acp_tool_call_permission_waiting_from_runtime(
                permission_tool_call_item_index,
                true,
            );
            model.show_acp_permission_request_with_preview(AcpPermissionPanelRequest {
                request_id: request.request_id.clone(),
                tool_call_id: Some(transcript_tool_call.tool_call_id.clone()),
                title: request.title.clone(),
                allow_option_id: acp_permission_option_id_for(
                    &request,
                    AcpPermissionOptionKind::AllowOnce,
                ),
                allow_always_option_id: acp_permission_option_id_for(
                    &request,
                    AcpPermissionOptionKind::AllowAlways,
                ),
                reject_option_id: acp_permission_option_id_for(
                    &request,
                    AcpPermissionOptionKind::RejectOnce,
                ),
                reject_always_option_id: acp_permission_option_id_for(
                    &request,
                    AcpPermissionOptionKind::RejectAlways,
                ),
                preview,
                tool_call_item_index: Some(permission_tool_call_item_index),
            });
            if let Some(item_index) = suspended_tool_call_item_index {
                model.suspend_acp_tool_call_for_approval_panel(item_index);
            }
        }
        AcpSessionEvent::TerminalUpdated { snapshot, .. } => {
            acp_ui_state.observe_terminal_snapshot(&snapshot);
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            let _ = model.apply_acp_terminal_snapshot_from_runtime(snapshot);
        }
        AcpSessionEvent::PermissionRequestCancelled { agent_id } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            model.apply_runtime_event(RuntimeEvent::PermissionCancelled {
                target: RuntimeTarget::acp_agent(agent_id),
                request_id: None,
            });
        }
        AcpSessionEvent::Stopped { agent_id, message } => {
            model.clear_acp_available_commands(&agent_id);
            if acp_ui_state.should_discard_prompt_output() {
                acp_ui_state.mark_prompt_finished();
                model.clear_stream_activity();
                return;
            }
            flush_acp_response_buffer(model, acp_ui_state);
            fail_tracked_acp_tool_calls(
                model,
                acp_ui_state,
                "Tool call ended because the ACP session stopped",
            );
            acp_ui_state.mark_prompt_finished();
            model.clear_stream_activity();
            if let Some(message) = message {
                model.show_transient_status_notice(&format!("ACP session stopped: {message}"));
            }
        }
    }
}

fn acp_permission_transcript_tool_call_update(request: &AcpPermissionRequest) -> AcpToolCallUpdate {
    let mut update = request.tool_call.clone();
    if is_execute_like_tool_call_update(&update, request.title.as_deref()) {
        update.content = update.content.map(|content| {
            content
                .into_iter()
                .filter(|content| !matches!(content, AcpToolCallContent::Text(_)))
                .collect()
        });
    }
    update
}

fn is_execute_like_tool_call_update(
    update: &AcpToolCallUpdate,
    fallback_title: Option<&str>,
) -> bool {
    update.kind == Some(AcpToolKind::Execute)
        || update
            .title
            .as_deref()
            .or(fallback_title)
            .is_some_and(is_execute_like_tool_title)
        || update
            .raw_input
            .as_ref()
            .and_then(|raw_input| raw_input.string_field(&["command", "cmd"]))
            .is_some()
}

fn is_execute_like_tool_title(title: &str) -> bool {
    let title = title.trim_start();
    title.starts_with("Shell:") || title.starts_with("Run ")
}

fn acp_permission_option_id_for(
    request: &AcpPermissionRequest,
    kind: AcpPermissionOptionKind,
) -> Option<String> {
    request
        .options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}

fn acp_tool_call_token_text(call: &AcpToolCall) -> Option<String> {
    acp_tool_call_projected_token_text(
        call.raw_input.as_ref().and_then(|raw| raw.token_text()),
        Some(call.content.as_slice()),
        call.raw_output.as_ref().and_then(|raw| raw.token_text()),
    )
}

fn acp_tool_call_update_token_text(update: &AcpToolCallUpdate) -> Option<String> {
    acp_tool_call_projected_token_text(
        update.raw_input.as_ref().and_then(|raw| raw.token_text()),
        update.content.as_deref(),
        update.raw_output.as_ref().and_then(|raw| raw.token_text()),
    )
}

fn acp_tool_call_projected_token_text(
    raw_input: Option<String>,
    content: Option<&[AcpToolCallContent]>,
    raw_output: Option<String>,
) -> Option<String> {
    if let Some(raw_input) = raw_input.filter(|text| !text.is_empty()) {
        return Some(raw_input);
    }

    let mut text = String::new();
    if let Some(content) = content {
        for content in content {
            append_acp_tool_call_content_token_text(&mut text, content);
        }
    }
    if let Some(raw_output) = raw_output.filter(|text| !text.is_empty()) {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&raw_output);
    }

    (!text.is_empty()).then_some(text)
}

fn append_acp_tool_call_content_token_text(text: &mut String, content: &AcpToolCallContent) {
    match content {
        AcpToolCallContent::Text(value) => append_token_text_segment(text, value),
        AcpToolCallContent::Resource {
            text: Some(value), ..
        } => append_token_text_segment(text, value),
        AcpToolCallContent::Diff {
            path,
            old_text,
            new_text,
        } => {
            append_token_text_segment(text, path);
            if let Some(old_text) = old_text {
                append_token_text_segment(text, old_text);
            }
            append_token_text_segment(text, new_text);
        }
        AcpToolCallContent::Image { mime_type, uri } => {
            append_token_text_segment(text, mime_type);
            if let Some(uri) = uri {
                append_token_text_segment(text, uri);
            }
        }
        AcpToolCallContent::Audio { mime_type } => append_token_text_segment(text, mime_type),
        AcpToolCallContent::ResourceLink { uri, name, title } => {
            append_token_text_segment(text, uri);
            append_token_text_segment(text, name);
            if let Some(title) = title {
                append_token_text_segment(text, title);
            }
        }
        AcpToolCallContent::Terminal { terminal_id } => {
            append_token_text_segment(text, terminal_id)
        }
        AcpToolCallContent::Resource { text: None, .. } | AcpToolCallContent::Unknown(_) => {}
    }
}

fn append_token_text_segment(text: &mut String, segment: &str) {
    if segment.is_empty() {
        return;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(segment);
}

fn upsert_acp_tool_call(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    call: AcpToolCall,
) -> usize {
    let tool_call_id = call.tool_call_id.clone();
    acp_ui_state.track_tool_call_terminal_content(&tool_call_id, Some(call.content.as_slice()));
    let item_index = match model.acp_tool_call_item_index_from_runtime(&tool_call_id) {
        Some(item_index) => {
            let update = acp_tool_call_update_from_call(call);
            model.update_acp_tool_call_from_runtime(item_index, update);
            item_index
        }
        None => model.append_acp_tool_call_from_runtime(call),
    };
    acp_ui_state.track_tool_call(tool_call_id, item_index);
    item_index
}

fn upsert_acp_tool_call_update(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    update: AcpToolCallUpdate,
    fallback_title: Option<String>,
) -> usize {
    let tool_call_id = update.tool_call_id.clone();
    acp_ui_state.track_tool_call_terminal_content(&tool_call_id, update.content.as_deref());
    let item_index = acp_ui_state
        .tool_call_item_index(&tool_call_id)
        .or_else(|| model.acp_tool_call_item_index_from_runtime(&tool_call_id));

    let item_index = match item_index {
        Some(item_index) => {
            model.update_acp_tool_call_from_runtime(item_index, update);
            item_index
        }
        None => model
            .append_acp_tool_call_from_runtime(acp_tool_call_from_update(update, fallback_title)),
    };
    acp_ui_state.track_tool_call(tool_call_id, item_index);
    item_index
}

fn acp_tool_call_update_from_call(call: AcpToolCall) -> AcpToolCallUpdate {
    AcpToolCallUpdate {
        tool_call_id: call.tool_call_id,
        title: Some(call.title),
        kind: Some(call.kind),
        status: Some(call.status),
        content: Some(call.content),
        locations: Some(call.locations),
        raw_input: call.raw_input,
        raw_output: call.raw_output,
    }
}

fn acp_tool_call_from_update(
    update: AcpToolCallUpdate,
    fallback_title: Option<String>,
) -> AcpToolCall {
    let tool_call_id = update.tool_call_id;
    AcpToolCall {
        title: update
            .title
            .or(fallback_title)
            .unwrap_or_else(|| format!("Tool call {tool_call_id}")),
        tool_call_id,
        kind: update.kind.unwrap_or(AcpToolKind::Other),
        status: update.status.unwrap_or(AcpToolCallStatus::Pending),
        content: update.content.unwrap_or_default(),
        locations: update.locations.unwrap_or_default(),
        raw_input: update.raw_input,
        raw_output: update.raw_output,
    }
}

fn fail_tracked_acp_tool_calls(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    message: &str,
) {
    let active_tool_call_indices = acp_ui_state.tracked_non_background_tool_call_indices();
    if active_tool_call_indices.is_empty() {
        acp_ui_state.clear_tool_call_tracking();
        return;
    }

    model.mark_acp_tool_calls_failed_from_runtime(active_tool_call_indices, message);
    acp_ui_state.clear_tool_call_tracking();
}

fn flush_acp_response_buffer(model: &mut Model, acp_ui_state: &mut AcpSessionUiState) {
    let content = acp_ui_state.take_response_buffer();
    let (reasoning_content, reasoning_duration) = acp_ui_state.take_reasoning_buffer();
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
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    agent_id: &str,
) {
    model.clear_acp_available_commands(agent_id);

    match runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::start(RuntimeTarget::acp_agent(agent_id)))
    {
        Ok(RuntimeCommandReceipt::AcpSessionStarted { default_model }) => {
            acp_ui_state.reset_for_new_session();
            model.set_acp_current_model(default_model);
            model.show_transient_status_notice(&format!("Starting ACP agent: {agent_id}"));
        }
        Ok(_) => {}
        Err(message) => {
            model.show_transient_status_notice(&message);
        }
    }
}

pub(super) fn run_send_acp_prompt_effect(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    submission: AcpPromptSubmission,
) {
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::submit_acp_prompt(submission))
    {
        model.clear_stream_activity();
        model.show_transient_status_notice(&message);
    } else {
        acp_ui_state.mark_prompt_submitted();
    }
}

pub(super) fn run_respond_acp_permission_effect(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    request_id: &str,
    option_id: Option<String>,
    is_rejection: bool,
    rejected_tool_call_id: Option<String>,
) {
    if is_rejection {
        acp_ui_state.suppress_rejected_permission_notice_for_tool_call(rejected_tool_call_id);
    }

    let target = model
        .selected_acp_agent()
        .map(|agent_id| RuntimeTarget::acp_agent(agent_id.to_string()));
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::RespondPermission {
            target,
            request_id: request_id.to_string(),
            option_id,
        })
    {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn run_set_acp_model_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    config_id: Option<String>,
    value: String,
) {
    let target = model
        .selected_acp_agent()
        .map(|agent_id| RuntimeTarget::acp_agent(agent_id.to_string()));
    if let Err(message) =
        runtime_coordinator.dispatch_runtime_command(RuntimeCommand::SetConfigOption {
            target,
            config_id,
            value,
        })
    {
        if let Some(agent_id) = model.selected_acp_agent().map(str::to_string) {
            model.rollback_pending_acp_model_change(&agent_id);
        }
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn run_stop_acp_background_terminals_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
) {
    let target = model
        .selected_acp_agent()
        .map(|agent_id| RuntimeTarget::acp_agent(agent_id.to_string()));
    if let Err(message) = runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::stop_background_terminals(target))
    {
        model.show_transient_status_notice(&message);
    }
}

pub(super) fn run_interrupt_acp_prompt_effect(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    should_dispatch_cancel: bool,
) {
    if !acp_ui_state.interrupt_prompt() {
        return;
    }

    if should_dispatch_cancel {
        let target = model
            .selected_acp_agent()
            .map(|agent_id| RuntimeTarget::acp_agent(agent_id.to_string()));
        let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::Interrupt { target });
    }
    if let Some(pending) = model.pending_acp_permission.take() {
        let target = model
            .selected_acp_agent()
            .map(|agent_id| RuntimeTarget::acp_agent(agent_id.to_string()));
        let _ = runtime_coordinator.dispatch_runtime_command(RuntimeCommand::RespondPermission {
            target,
            request_id: pending.request_id,
            option_id: None,
        });
        model.close_tool_approval_panel();
    }
    fail_tracked_acp_tool_calls(model, acp_ui_state, "Interrupted");
    model.clear_stream_activity();
    model.append_system_message_from_runtime("Chat interrupted");
}

pub(super) fn acp_reject_option_id_for_stale_discard(
    request: &mo_core::acp::AcpPermissionRequest,
) -> Option<String> {
    use mo_core::acp::AcpPermissionOptionKind;

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
