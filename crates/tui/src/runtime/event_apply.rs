use mo_core::session::{
    RuntimeEvent, RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeToolActivity,
    RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
};

use super::super::{
    Model, model::RequestMetrics, runtime_tool_preview::ToolApprovalPreview,
    tool_approval_panel::ToolApprovalSource,
};

const FALLBACK_CHAT_FAILURE_MESSAGE: &str = "Unknown error";

pub(crate) trait RuntimeEventApply {
    fn apply_runtime_event(&mut self, event: RuntimeEvent);
}

impl RuntimeEventApply for Model {
    fn apply_runtime_event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Started { identity, .. } => {
                self.show_transient_status_notice(&format!("Runtime ready: {}", identity.label));
            }
            RuntimeEvent::StartFailed { message, .. } => {
                self.show_transient_status_notice(&format!("Runtime start failed: {message}"));
            }
            RuntimeEvent::SystemMessage { message, .. } => {
                self.flush_runtime_response_buffer();
                self.append_system_message_from_runtime(message);
            }
            RuntimeEvent::TurnStarted { label, .. } => {
                self.clear_runtime_response_buffer();
                if self.stream_activity.is_none() {
                    self.show_stream_activity(label);
                }
            }
            RuntimeEvent::AssistantDelta { content, .. } => {
                self.push_runtime_assistant_delta(&content);
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::ReasoningDelta { content, .. } => {
                self.push_runtime_reasoning_delta(&content);
                self.set_stream_activity_thinking(true);
            }
            RuntimeEvent::OutputTokenEstimate { total_tokens, .. } => {
                self.set_stream_activity_output_tokens(total_tokens);
            }
            RuntimeEvent::InputTokenEstimate { total_tokens, .. } => {
                self.set_stream_activity_input_tokens(total_tokens);
            }
            RuntimeEvent::Thinking { is_thinking, .. } => {
                self.set_stream_activity_thinking(is_thinking);
            }
            RuntimeEvent::Retrying { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.clear_runtime_response_buffer();
                self.show_stream_activity_with_header(message);
            }
            RuntimeEvent::ToolActivityStarted { activity, .. } => {
                self.flush_runtime_response_buffer();
                self.append_runtime_tool_activity_from_runtime(activity);
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::ToolActivityUpdated { update, .. } => {
                self.flush_runtime_response_buffer();
                upsert_runtime_tool_activity(self, update);
                self.set_stream_activity_thinking(false);
            }
            RuntimeEvent::TerminalUpdated { snapshot, .. } => {
                let _ = self.apply_runtime_terminal_snapshot_from_runtime(snapshot);
            }
            RuntimeEvent::PermissionRequested { target, request } => {
                self.flush_runtime_response_buffer();
                show_runtime_permission_request(self, target, request);
            }
            RuntimeEvent::PermissionCancelled { .. } => {
                self.close_tool_approval_panel();
                self.show_transient_status_notice("Runtime permission request cancelled");
            }
            RuntimeEvent::MessageFinished {
                content,
                reasoning_content,
                reasoning_duration,
                metrics,
                ..
            } => {
                self.close_runtime_permission_approval_panel();
                if let Some(metrics) = metrics {
                    self.set_last_request_metrics(Some(RequestMetrics::new(
                        metrics.latency,
                        metrics.output_tokens,
                        metrics.duration,
                    )));
                }
                self.set_stream_activity_thinking(false);
                self.flush_runtime_response_buffer_with_final(
                    content,
                    reasoning_content,
                    reasoning_duration,
                );
                self.finish_stream_activity_with_work_summary();
            }
            RuntimeEvent::Failed { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.append_system_message_from_runtime(normalize_chat_failure_message(&message));
                self.finish_stream_activity_with_work_summary();
            }
            RuntimeEvent::Interrupted { .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.append_system_message_from_runtime("Chat interrupted");
                self.finish_stream_activity_with_work_summary();
            }
            RuntimeEvent::Stopped { message, .. } => {
                self.close_runtime_permission_approval_panel();
                self.flush_runtime_response_buffer();
                self.accept_streamed_runtime_reasoning_from_runtime();
                self.finish_stream_activity_with_work_summary();
                if let Some(message) = message {
                    self.show_transient_status_notice(&format!("Runtime stopped: {message}"));
                }
            }
        }
    }
}

fn normalize_chat_failure_message(message: &str) -> String {
    let normalized_message = message.trim();
    if normalized_message.is_empty() {
        return FALLBACK_CHAT_FAILURE_MESSAGE.to_string();
    }

    let (description, json_body) = split_error_description_and_json_body(normalized_message);
    let description = normalize_error_description(&description);

    match (description.is_empty(), json_body) {
        (true, Some(body)) => format!("{FALLBACK_CHAT_FAILURE_MESSAGE}\nBody: {body}"),
        (true, None) => FALLBACK_CHAT_FAILURE_MESSAGE.to_string(),
        (false, Some(body)) => format!("{description}\nBody: {body}"),
        (false, None) => description,
    }
}

fn split_error_description_and_json_body(message: &str) -> (String, Option<String>) {
    let message_lines = message.lines().collect::<Vec<_>>();
    let Some(last_non_empty_line_index) = message_lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
    else {
        return (String::new(), None);
    };

    let last_non_empty_line = message_lines[last_non_empty_line_index].trim();
    if let Some(body) = last_non_empty_line
        .strip_prefix("Body:")
        .map(str::trim)
        .filter(|body| is_json_body(body))
    {
        let description = message_lines[..last_non_empty_line_index].join("\n");
        return (description, Some(body.to_string()));
    }

    if is_json_body(last_non_empty_line) {
        let description = message_lines[..last_non_empty_line_index].join("\n");
        return (description, Some(last_non_empty_line.to_string()));
    }

    if let Some((description_suffix, body)) = split_inline_json_body(last_non_empty_line) {
        let mut description_lines = message_lines[..last_non_empty_line_index]
            .iter()
            .map(|line| (*line).to_string())
            .collect::<Vec<_>>();
        description_lines.push(description_suffix.to_string());
        return (description_lines.join("\n"), Some(body.to_string()));
    }

    (message.to_string(), None)
}

fn split_inline_json_body(line: &str) -> Option<(&str, &str)> {
    for (index, character) in line.char_indices() {
        if !matches!(character, '{' | '[') {
            continue;
        }

        let body = line[index..].trim();
        if is_json_body(body) {
            return Some((line[..index].trim_end(), body));
        }
    }

    None
}

fn is_json_body(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body).is_ok()
}

fn normalize_error_description(description: &str) -> String {
    let mut lines = description.lines();
    let Some(first_line) = lines.next() else {
        return String::new();
    };

    let mut normalized_lines = vec![first_line.trim().to_string()];
    normalized_lines.extend(
        lines
            .map(str::trim_end)
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string),
    );
    normalized_lines.join("\n")
}

fn upsert_runtime_tool_activity(model: &mut Model, update: RuntimeToolActivityUpdate) {
    let activity_id = update.activity_id.clone();
    match model.runtime_tool_activity_item_index_from_runtime(&activity_id) {
        Some(item_index) => {
            model.update_runtime_tool_activity_from_runtime(item_index, update);
        }
        None => {
            model.append_runtime_tool_activity_from_runtime(runtime_tool_activity_from_update(
                update,
            ));
        }
    }
}

fn runtime_tool_activity_from_update(update: RuntimeToolActivityUpdate) -> RuntimeToolActivity {
    let activity_id = update.activity_id;
    let title = update
        .title
        .unwrap_or_else(|| format!("Tool activity {activity_id}"));
    RuntimeToolActivity {
        activity_id,
        title,
        kind: update.kind.unwrap_or(RuntimeToolKind::Other),
        status: update.status.unwrap_or(RuntimeToolActivityStatus::Pending),
        content: update.content.unwrap_or_default(),
        locations: update.locations.unwrap_or_default(),
        raw_input: update.raw_input,
        raw_output: update.raw_output,
    }
}

fn show_runtime_permission_request(
    model: &mut Model,
    target: mo_core::session::RuntimeTarget,
    request: RuntimePermissionRequest,
) {
    let preview = request
        .tool_activity
        .as_ref()
        .and_then(ToolApprovalPreview::from_runtime_tool_activity_update);
    let title = request.title.as_deref().unwrap_or("");
    model.clear_status_notice();
    model.open_tool_approval_panel_with_preview(
        ToolApprovalSource::RuntimePermission {
            target,
            request_id: request.request_id,
            allow_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::AllowOnce,
            ),
            allow_always_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::AllowAlways,
            ),
            reject_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::RejectOnce,
            ),
            reject_always_option_id: option_id_for(
                &request.options,
                RuntimePermissionOptionKind::RejectAlways,
            ),
        },
        title.to_string(),
        Vec::new(),
        preview,
    );
}

fn option_id_for(
    options: &[mo_core::session::RuntimePermissionOption],
    kind: RuntimePermissionOptionKind,
) -> Option<String> {
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}

#[cfg(test)]
mod tests {
    use super::normalize_chat_failure_message;

    #[test]
    fn chat_failure_message_marks_json_body() {
        let message = "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\n{\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\nBody: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}"
        );
    }

    #[test]
    fn chat_failure_message_extracts_inline_json_body() {
        let message = "provider error HTTP 401: Invalid status code 401 Unauthorized with message: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 401: Invalid status code 401 Unauthorized with message:\nBody: {\"type\":\"error\",\"error\":{\"type\":\"CreditsError\",\"message\":\"Insufficient balance...\"}}"
        );
    }

    #[test]
    fn chat_failure_message_preserves_non_json_details() {
        let message =
            "provider error HTTP 400: HTTP error.\nStatus: 400 Bad Request\nCause: bad request";

        assert_eq!(
            normalize_chat_failure_message(message),
            "provider error HTTP 400: HTTP error.\nStatus: 400 Bad Request\nCause: bad request"
        );
    }
}
