use std::path::PathBuf;

use crate::frontend::tui::{Model, runtime::RuntimeEventApply};
use crate::runtime::native::{NativeAgentEvent, NativeAgentRequest, NativeAgentRuntimeState};
use crate::runtime::request_policy::RuntimeRequestPolicy;
use crate::runtime::session::{RuntimeEvent, RuntimeRequestMetrics, RuntimeTarget};
use crate::runtime::tools::builtin::workspace_readonly_tool_registry;
use crate::runtime::tools::{RuntimeToolCall, RuntimeToolExecutorRegistry, RuntimeToolResult};

pub(super) fn drain_native_agent_runtime_events(
    model: &mut Model,
    native_agent_runtime: &mut NativeAgentRuntimeState,
) -> bool {
    let mut changed = false;
    loop {
        let target = native_agent_runtime.current_target().cloned();
        let Some(event) = native_agent_runtime.try_recv_event() else {
            break;
        };
        apply_native_agent_event(model, target, event);
        changed = true;
    }
    changed
}

pub(super) fn apply_native_agent_event(
    model: &mut Model,
    target: Option<RuntimeTarget>,
    event: NativeAgentEvent,
) {
    let runtime_event = match event {
        NativeAgentEvent::Retrying { message } => {
            model.show_acp_activity_with_header(message);
            return;
        }
        NativeAgentEvent::OutputTokenEstimate { total_tokens } => {
            RuntimeEvent::OutputTokenEstimate {
                target,
                total_tokens,
            }
        }
        NativeAgentEvent::Thinking { is_thinking } => RuntimeEvent::Thinking {
            target,
            is_thinking,
        },
        NativeAgentEvent::ToolExecutionStarted { call } => {
            model.show_acp_activity_with_header(format!(
                "Running {}",
                native_agent_tool_label(&call)
            ));
            return;
        }
        NativeAgentEvent::ToolExecutionFinished { call, result } => {
            append_native_agent_tool_result(model, &call, &result);
            return;
        }
        NativeAgentEvent::Finished { response, metrics } => RuntimeEvent::MessageFinished {
            target,
            content: response.content,
            reasoning_content: response.reasoning_content,
            reasoning_duration: response.reasoning_duration,
            metrics: metrics.map(|metrics| {
                RuntimeRequestMetrics::new(metrics.latency, metrics.output_tokens, metrics.duration)
            }),
        },
        NativeAgentEvent::Failed { message } => RuntimeEvent::Failed { target, message },
        NativeAgentEvent::Interrupted => RuntimeEvent::Interrupted { target },
    };
    model.apply_runtime_event(runtime_event);
}

pub(super) fn run_send_native_agent_effect(
    model: &mut Model,
    native_agent_runtime: &mut NativeAgentRuntimeState,
    request: NativeAgentRequest,
    request_policy: RuntimeRequestPolicy,
) {
    if native_agent_runtime.is_running() {
        model.show_transient_status_notice("Chat request is already running");
        return;
    }

    let activity_label = request.llm_request().model_id.clone();
    let tools = native_agent_workspace_tools();
    let request = request.with_tools(tools.definitions());
    native_agent_runtime.start(request, tools, request_policy);
    model.show_acp_activity(activity_label);
}

pub(super) fn native_agent_workspace_tools() -> RuntimeToolExecutorRegistry {
    let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    workspace_readonly_tool_registry(root)
}

pub(super) fn run_interrupt_native_agent_effect(
    model: &mut Model,
    native_agent_runtime: &mut NativeAgentRuntimeState,
) -> bool {
    if native_agent_runtime.interrupt() {
        model.clear_acp_activity();
        model.append_system_message_from_runtime("Chat interrupted");
        return true;
    }
    false
}

fn append_native_agent_tool_result(
    model: &mut Model,
    call: &RuntimeToolCall,
    result: &RuntimeToolResult,
) {
    model.append_tool_result_from_runtime(
        native_agent_tool_result_content(call, result),
        crate::frontend::tui::tool_result::ToolResultKind::Ran,
    );
}

fn native_agent_tool_result_content(call: &RuntimeToolCall, result: &RuntimeToolResult) -> String {
    let mut content = format!("Ran {}", native_agent_tool_label(call));
    if result.is_error {
        content.push_str(": failed");
        if let Some(summary) = native_agent_tool_result_summary(&result.content) {
            content.push_str(" - ");
            content.push_str(&summary);
        }
    }
    content
}

fn native_agent_tool_label(call: &RuntimeToolCall) -> String {
    if let Some(path) = call
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
    {
        return format!("{} {}", call.name, path);
    }

    call.name.clone()
}

fn native_agent_tool_result_summary(content: &str) -> Option<String> {
    let first_line = content.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut summary = first_line.chars().take(120).collect::<String>();
    if first_line.chars().count() > 120 {
        summary.push_str("...");
    }
    Some(summary)
}
