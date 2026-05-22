use std::time::Instant;

#[cfg(test)]
use super::NoopRuntimeCoordinator;
use super::RuntimeCoordinator;
use crate::{
    AcpPromptSubmission, Model, acp::AcpPermissionPanelRequest,
    acp_tool_preview::ToolApprovalPreview, runtime::RuntimeEventApply,
};
use mo_core::acp::AcpAgentIdentity;
#[cfg(test)]
use mo_core::acp::AcpSessionEvent;
use mo_core::session::{
    RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, RuntimePermissionOptionKind,
    RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityContent,
    RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
    RuntimeToolKind,
};

mod state;

pub(super) use state::AcpSessionUiState;

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
    update: &mut RuntimeToolActivityUpdate,
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

fn is_agent_facing_permission_rejection_tool_content(content: &RuntimeToolActivityContent) -> bool {
    matches!(content, RuntimeToolActivityContent::Text(text) if is_agent_facing_permission_rejection_notice(text))
}

fn is_agent_facing_permission_rejection_raw_value(raw_value: &RuntimeToolActivityRawValue) -> bool {
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
    apply_runtime_event_with_coordinator(
        model,
        acp_ui_state,
        &mut runtime_coordinator,
        event.into_runtime_event(),
    );
}

pub(super) fn apply_runtime_event_with_coordinator(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    event: RuntimeEvent,
) {
    match event {
        RuntimeEvent::Started {
            target: RuntimeTarget::AcpAgent { agent_id },
            identity,
        } => {
            let display_name = identity.label.clone();
            model.apply_acp_agent_identity(
                agent_id,
                AcpAgentIdentity::from_runtime_identity(&identity),
            );
            model.show_transient_status_notice(&format!("ACP session ready: {display_name}"));
        }
        RuntimeEvent::StartFailed {
            target: Some(RuntimeTarget::AcpAgent { .. }),
            message,
        } => {
            model.show_transient_status_notice(&format!("ACP start failed: {message}"));
        }
        RuntimeEvent::SystemMessage {
            target: Some(RuntimeTarget::AcpAgent { .. }),
            message,
        } => {
            if !acp_ui_state.should_discard_prompt_output() {
                flush_acp_response_buffer(model, acp_ui_state);
            }
            model.append_system_message_from_runtime(message);
        }
        RuntimeEvent::TurnStarted {
            target: RuntimeTarget::AcpAgent { agent_id },
            label,
        } => {
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
                model.show_stream_activity(label);
            }
        }
        RuntimeEvent::ReasoningDelta {
            target: RuntimeTarget::AcpAgent { .. },
            content,
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            if !content.is_empty() && acp_ui_state.response_buffers_empty() {
                let _ = model.mark_exploration_tool_activities_complete_from_runtime();
            }
            acp_ui_state.push_reasoning_chunk(&content);
            model.set_stream_activity_thinking(true);
            if let Some(total_tokens) = acp_ui_state.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
            }
        }
        RuntimeEvent::AssistantDelta {
            target: RuntimeTarget::AcpAgent { .. },
            content,
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            if !content.is_empty() && acp_ui_state.response_buffers_empty() {
                let _ = model.mark_exploration_tool_activities_complete_from_runtime();
            }
            model.set_stream_activity_thinking(false);
            if model.streams_reasoning_into_transcript_during_response() {
                let (reasoning_content, reasoning_duration) = acp_ui_state.take_reasoning_buffer();
                if reasoning_content.is_some() {
                    model.append_acp_response_from_runtime(
                        String::new(),
                        reasoning_content,
                        reasoning_duration,
                    );
                }
            }
            acp_ui_state.push_response_chunk(&content);
            if let Some(total_tokens) = acp_ui_state.observe_output_tokens(&content) {
                model.set_stream_activity_output_tokens(total_tokens);
            }
        }
        RuntimeEvent::ToolActivityStarted {
            target: RuntimeTarget::AcpAgent { .. },
            activity: call,
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            flush_acp_response_buffer(model, acp_ui_state);
            acp_ui_state.track_tool_call_status(&call.activity_id, call.status);
            upsert_acp_tool_call(model, acp_ui_state, call);
            model.set_stream_activity_thinking(false);
        }
        RuntimeEvent::ToolActivityUpdated {
            target: RuntimeTarget::AcpAgent { .. },
            mut update,
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            flush_acp_response_buffer(model, acp_ui_state);
            let tool_call_id = update.activity_id.clone();
            sanitize_rejected_permission_tool_call_update(
                &mut update,
                acp_ui_state.should_sanitize_rejected_permission_tool_update(&tool_call_id),
            );
            let terminal_status = update
                .status
                .or_else(|| acp_ui_state.tool_call_status(&tool_call_id));
            if let Some(total_tokens) = acp_ui_state.observe_tool_result_tokens(
                &tool_call_id,
                acp_tool_result_token_text(&update, terminal_status),
            ) {
                model.set_stream_activity_input_tokens(total_tokens);
            }
            if let Some(status) = update.status {
                acp_ui_state.track_tool_call_status(&tool_call_id, status);
            }
            upsert_acp_tool_call_update(model, acp_ui_state, update, None);
            model.set_stream_activity_thinking(false);
        }
        RuntimeEvent::ModelConfigChanged {
            target: RuntimeTarget::AcpAgent { agent_id },
            config,
        } => {
            model.apply_acp_model_config(&agent_id, config);
        }
        RuntimeEvent::AvailableCommandsChanged {
            target: RuntimeTarget::AcpAgent { agent_id },
            commands,
        } => {
            model.apply_acp_available_commands(agent_id, commands);
        }
        RuntimeEvent::ConfigChangeSucceeded {
            target: RuntimeTarget::AcpAgent { agent_id },
        } => {
            model.commit_pending_acp_model_change(&agent_id);
        }
        RuntimeEvent::ConfigChangeFailed {
            target: RuntimeTarget::AcpAgent { agent_id },
            message,
        } => {
            model.rollback_pending_acp_model_change(&agent_id);
            model.show_transient_status_notice(&format!("ACP config change failed: {message}"));
        }
        RuntimeEvent::MessageFinished {
            target: Some(RuntimeTarget::AcpAgent { .. }),
            content,
            finish_reason,
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
            model.finish_stream_activity_with_work_summary();
            if let Some(finish_reason) = finish_reason {
                model
                    .show_transient_status_notice(&format!("ACP prompt finished: {finish_reason}"));
            }
        }
        RuntimeEvent::Failed {
            target: Some(RuntimeTarget::AcpAgent { .. }),
            message,
        } => {
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
            model.finish_stream_activity_with_work_summary();
            model.show_transient_status_notice(&format!("ACP prompt failed: {message}"));
        }
        RuntimeEvent::Interrupted {
            target: Some(RuntimeTarget::AcpAgent { .. }),
        } => {
            fail_tracked_acp_tool_calls(model, acp_ui_state, "Interrupted");
            acp_ui_state.mark_prompt_finished();
            model.finish_stream_activity_with_work_summary();
        }
        RuntimeEvent::PermissionRequested {
            target: RuntimeTarget::AcpAgent { agent_id },
            request,
        } => {
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
            let permission_tool_call_item_index = transcript_tool_call.as_ref().map(|update| {
                upsert_acp_tool_call_update(
                    model,
                    acp_ui_state,
                    update.clone(),
                    request.title.clone(),
                )
            });
            let preview = transcript_tool_call
                .as_ref()
                .and_then(ToolApprovalPreview::from_runtime_tool_activity_update);
            let suspended_tool_call_item_index =
                preview.as_ref().and(permission_tool_call_item_index);
            if let Some(item_index) = permission_tool_call_item_index {
                model.set_acp_tool_call_permission_waiting_from_runtime(item_index, true);
            }
            model.show_acp_permission_request_with_preview(AcpPermissionPanelRequest {
                request_id: request.request_id.clone(),
                tool_call_id: transcript_tool_call
                    .as_ref()
                    .map(|update| update.activity_id.clone()),
                title: request.title.clone(),
                allow_option_id: acp_permission_option_id_for(
                    &request,
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                allow_always_option_id: acp_permission_option_id_for(
                    &request,
                    RuntimePermissionOptionKind::AllowAlways,
                ),
                reject_option_id: acp_permission_option_id_for(
                    &request,
                    RuntimePermissionOptionKind::RejectOnce,
                ),
                reject_always_option_id: acp_permission_option_id_for(
                    &request,
                    RuntimePermissionOptionKind::RejectAlways,
                ),
                preview,
                tool_call_item_index: permission_tool_call_item_index,
            });
            if let Some(item_index) = suspended_tool_call_item_index {
                model.suspend_acp_tool_call_for_approval_panel(item_index);
            }
        }
        RuntimeEvent::TerminalUpdated {
            target: RuntimeTarget::AcpAgent { .. },
            snapshot,
        } => {
            acp_ui_state.observe_terminal_snapshot(&snapshot);
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            let _ = model.apply_runtime_terminal_snapshot_from_runtime(snapshot);
        }
        RuntimeEvent::PermissionCancelled {
            target: RuntimeTarget::AcpAgent { .. },
            ..
        } => {
            if acp_ui_state.should_discard_prompt_output() {
                return;
            }
            model.close_tool_approval_panel();
            model.show_transient_status_notice("ACP permission request cancelled");
        }
        RuntimeEvent::Stopped {
            target: RuntimeTarget::AcpAgent { agent_id },
            message,
        } => {
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
            model.finish_stream_activity_with_work_summary();
            if let Some(message) = message {
                model.show_transient_status_notice(&format!("ACP session stopped: {message}"));
            }
        }
        event => model.apply_runtime_event(event),
    }
}

fn acp_permission_transcript_tool_call_update(
    request: &RuntimePermissionRequest,
) -> Option<RuntimeToolActivityUpdate> {
    let mut update = request.tool_activity.clone()?;
    if is_execute_like_tool_call_update(&update, request.title.as_deref()) {
        update.content = update.content.map(|content| {
            content
                .into_iter()
                .filter(|content| !matches!(content, RuntimeToolActivityContent::Text(_)))
                .collect()
        });
    }
    Some(update)
}

fn is_execute_like_tool_call_update(
    update: &RuntimeToolActivityUpdate,
    fallback_title: Option<&str>,
) -> bool {
    update.kind == Some(RuntimeToolKind::Execute)
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
    request: &RuntimePermissionRequest,
    kind: RuntimePermissionOptionKind,
) -> Option<String> {
    request
        .options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}

fn acp_tool_result_token_text(
    update: &RuntimeToolActivityUpdate,
    terminal_status: Option<RuntimeToolActivityStatus>,
) -> Option<String> {
    if !matches!(
        terminal_status,
        Some(RuntimeToolActivityStatus::Completed | RuntimeToolActivityStatus::Failed)
    ) {
        return None;
    }

    if let Some(raw_output) = update
        .raw_output
        .as_ref()
        .and_then(|raw| raw.token_text())
        .filter(|text| !text.is_empty())
    {
        return Some(raw_output);
    }

    acp_tool_call_content_token_text(update.content.as_deref())
}

fn acp_tool_call_content_token_text(
    content: Option<&[RuntimeToolActivityContent]>,
) -> Option<String> {
    let mut text = String::new();
    if let Some(content) = content {
        for content in content {
            append_acp_tool_call_content_token_text(&mut text, content);
        }
    }

    (!text.is_empty()).then_some(text)
}

fn append_acp_tool_call_content_token_text(
    text: &mut String,
    content: &RuntimeToolActivityContent,
) {
    match content {
        RuntimeToolActivityContent::Text(value) => append_token_text_segment(text, value),
        RuntimeToolActivityContent::Resource {
            text: Some(value), ..
        } => append_token_text_segment(text, value),
        RuntimeToolActivityContent::Diff {
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
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            append_token_text_segment(text, mime_type);
            if let Some(uri) = uri {
                append_token_text_segment(text, uri);
            }
        }
        RuntimeToolActivityContent::Audio { mime_type } => {
            append_token_text_segment(text, mime_type)
        }
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            append_token_text_segment(text, uri);
            append_token_text_segment(text, name);
            if let Some(title) = title {
                append_token_text_segment(text, title);
            }
        }
        RuntimeToolActivityContent::Terminal { terminal_id } => {
            append_token_text_segment(text, terminal_id)
        }
        RuntimeToolActivityContent::Resource { text: None, .. }
        | RuntimeToolActivityContent::Unknown(_) => {}
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
    call: RuntimeToolActivity,
) -> usize {
    let tool_call_id = call.activity_id.clone();
    acp_ui_state.track_tool_call_terminal_content(&tool_call_id, Some(call.content.as_slice()));
    let item_index = match model.runtime_tool_activity_item_index_from_runtime(&tool_call_id) {
        Some(item_index) => {
            let update = acp_tool_call_update_from_call(call);
            model.update_runtime_tool_activity_from_runtime(item_index, update);
            item_index
        }
        None => model.append_runtime_tool_activity_from_runtime(call),
    };
    acp_ui_state.track_tool_call(tool_call_id, item_index);
    item_index
}

fn upsert_acp_tool_call_update(
    model: &mut Model,
    acp_ui_state: &mut AcpSessionUiState,
    update: RuntimeToolActivityUpdate,
    fallback_title: Option<String>,
) -> usize {
    let tool_call_id = update.activity_id.clone();
    acp_ui_state.track_tool_call_terminal_content(&tool_call_id, update.content.as_deref());
    let item_index = acp_ui_state
        .tool_call_item_index(&tool_call_id)
        .or_else(|| model.runtime_tool_activity_item_index_from_runtime(&tool_call_id));

    let item_index = match item_index {
        Some(item_index) => {
            model.update_runtime_tool_activity_from_runtime(item_index, update);
            item_index
        }
        None => model.append_runtime_tool_activity_from_runtime(acp_tool_call_from_update(
            update,
            fallback_title,
        )),
    };
    acp_ui_state.track_tool_call(tool_call_id, item_index);
    item_index
}

fn acp_tool_call_update_from_call(call: RuntimeToolActivity) -> RuntimeToolActivityUpdate {
    RuntimeToolActivityUpdate {
        activity_id: call.activity_id,
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
    update: RuntimeToolActivityUpdate,
    fallback_title: Option<String>,
) -> RuntimeToolActivity {
    let tool_call_id = update.activity_id;
    let title = update
        .title
        .or(fallback_title)
        .unwrap_or_else(|| format!("Tool call {tool_call_id}"));
    RuntimeToolActivity {
        activity_id: tool_call_id,
        title,
        kind: update.kind.unwrap_or(RuntimeToolKind::Other),
        status: update.status.unwrap_or(RuntimeToolActivityStatus::Pending),
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
    model.append_system_message_from_runtime("Chat interrupted");
    model.finish_stream_activity_with_work_summary();
}

pub(super) fn acp_reject_option_id_for_stale_discard(
    request: &RuntimePermissionRequest,
) -> Option<String> {
    request
        .options
        .iter()
        .find(|option| option.kind == RuntimePermissionOptionKind::RejectOnce)
        .or_else(|| {
            request
                .options
                .iter()
                .find(|option| option.kind == RuntimePermissionOptionKind::RejectAlways)
        })
        .map(|option| option.option_id.clone())
}
