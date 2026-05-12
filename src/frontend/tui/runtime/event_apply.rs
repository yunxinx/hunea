use crate::runtime::session::{
    RuntimeEvent, RuntimePermissionOptionKind, RuntimePermissionRequest,
};

use super::super::{AppEvent, Model, model::RequestMetrics};

pub(in crate::frontend::tui) trait RuntimeEventApply {
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
                self.append_system_message_from_runtime(message);
            }
            RuntimeEvent::TurnStarted { label, .. } => {
                if self.stream_activity.is_none() {
                    self.show_stream_activity(label);
                }
            }
            RuntimeEvent::AssistantDelta { .. } | RuntimeEvent::ReasoningDelta { .. } => {}
            RuntimeEvent::OutputTokenEstimate { total_tokens, .. } => {
                self.set_stream_activity_output_tokens(total_tokens);
            }
            RuntimeEvent::Thinking { is_thinking, .. } => {
                self.set_stream_activity_thinking(is_thinking);
            }
            RuntimeEvent::PermissionRequested { request, .. } => {
                show_runtime_permission_request(self, request);
            }
            RuntimeEvent::PermissionCancelled { target, .. } => {
                self.close_tool_approval_panel();
                let message = match target {
                    crate::runtime::session::RuntimeTarget::AcpAgent { .. } => {
                        "ACP permission request cancelled"
                    }
                    _ => "Runtime permission request cancelled",
                };
                self.show_transient_status_notice(message);
            }
            RuntimeEvent::MessageFinished {
                content,
                reasoning_content,
                reasoning_duration,
                metrics,
                ..
            } => {
                if let Some(metrics) = metrics {
                    self.set_last_request_metrics(Some(RequestMetrics::new(
                        metrics.latency,
                        metrics.output_tokens,
                        metrics.duration,
                    )));
                }
                self.clear_stream_activity();
                self.append_runtime_response_from_runtime(
                    content,
                    reasoning_content,
                    reasoning_duration,
                );
            }
            RuntimeEvent::Failed { message, .. } => {
                self.clear_stream_activity();
                self.append_system_message_from_runtime(format!("Chat failed: {message}"));
            }
            RuntimeEvent::Interrupted { .. } => {
                self.clear_stream_activity();
                self.append_system_message_from_runtime("Chat interrupted");
            }
            RuntimeEvent::Stopped { message, .. } => {
                self.clear_stream_activity();
                if let Some(message) = message {
                    self.show_transient_status_notice(&format!("Runtime stopped: {message}"));
                }
            }
        }
    }
}

fn show_runtime_permission_request(model: &mut Model, request: RuntimePermissionRequest) {
    model.update(AppEvent::AcpPermissionRequested {
        request_id: request.request_id,
        title: request.title,
        allow_option_id: option_id_for(&request.options, RuntimePermissionOptionKind::AllowOnce),
        allow_always_option_id: option_id_for(
            &request.options,
            RuntimePermissionOptionKind::AllowAlways,
        ),
        reject_option_id: option_id_for(&request.options, RuntimePermissionOptionKind::RejectOnce),
        reject_always_option_id: option_id_for(
            &request.options,
            RuntimePermissionOptionKind::RejectAlways,
        ),
    });
}

fn option_id_for(
    options: &[crate::runtime::session::RuntimePermissionOption],
    kind: RuntimePermissionOptionKind,
) -> Option<String> {
    options
        .iter()
        .find(|option| option.kind == kind)
        .map(|option| option.option_id.clone())
}
