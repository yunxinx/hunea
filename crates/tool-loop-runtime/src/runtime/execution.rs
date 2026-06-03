use provider_protocol::{
    ToolCall as AiToolCall, ToolDefinition as AiToolDefinition, ToolResult as AiToolResult,
};
use runtime_domain::session::{
    ManagedSearchTool, RuntimeTerminalExitStatus, RuntimeTerminalSnapshot,
};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ProcessedToolError, SharedToolErrorFormatter, SharedToolPermissionHandler,
    ToolExecutionContext, ToolExecutor, ToolExecutorRegistry, ToolKind, ToolPermissionDecision,
    ToolPermissionFileSnapshot, ToolPermissionPolicy, ToolPermissionPreview, ToolPermissionRequest,
    ToolProgress, ToolProgressSink, ToolRegistry, ToolResult, ToolTerminalExitStatus,
    ToolTerminalSnapshot,
};

use super::{ToolLoopClock, ToolLoopProgress, state::RuntimeTurnState};

const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";

pub(super) struct ToolExecution {
    pub(super) raw_result: ToolResult,
    pub(super) provider_result: AiToolResult,
    pub(super) processed_error: Option<ProcessedToolError>,
}

pub(super) fn interrupted_tool_execution(call: &AiToolCall) -> ToolExecution {
    let processed_error =
        ProcessedToolError::new(TOOL_EXECUTION_INTERRUPTED, TOOL_EXECUTION_INTERRUPTED);
    ToolExecution {
        raw_result: ToolResult::error(call.call_id.clone(), TOOL_EXECUTION_INTERRUPTED),
        provider_result: AiToolResult::error(
            call.call_id.clone(),
            call.name.clone(),
            processed_error.assistant_message.clone(),
            None,
        ),
        processed_error: Some(processed_error),
    }
}

pub(super) async fn execute_tool_call(
    call: &AiToolCall,
    context: &mut ToolCallExecutionContext<'_>,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) -> ToolExecution {
    let runtime_call = tool_runtime::ToolCall::new(
        call.call_id.clone(),
        call.name.clone(),
        call.arguments.clone(),
    );

    let authorization = authorize_tool_call(&runtime_call, context).await;
    let raw_result = match authorization.denial_message {
        Some(message) => ToolResult::error(call.call_id.clone(), message),
        None => {
            execute_tool_with_progress(
                runtime_call,
                authorization.permission_snapshot,
                context,
                on_progress,
            )
            .await
        }
    };

    let processed_error = (raw_result.is_error
        && !is_command_execution_error(
            &raw_result,
            context.tool_definitions.definition(&call.name),
        ))
    .then(|| {
        context
            .error_formatter
            .format_tool_error(&call.name, &raw_result.content)
    });
    let provider_content = processed_error
        .as_ref()
        .map(|processed| processed.assistant_message.clone())
        .unwrap_or_else(|| raw_result.content.clone());
    let provider_result = if raw_result.is_error {
        AiToolResult::error(
            call.call_id.clone(),
            call.name.clone(),
            provider_content,
            raw_result.details.clone(),
        )
    } else {
        AiToolResult::success(
            call.call_id.clone(),
            call.name.clone(),
            provider_content,
            raw_result.details.clone(),
        )
    };

    ToolExecution {
        raw_result,
        provider_result,
        processed_error,
    }
}

fn is_command_execution_error(
    result: &ToolResult,
    definition: Option<&tool_runtime::ToolDefinition>,
) -> bool {
    if definition.map(|definition| definition.kind) != Some(ToolKind::Execute) {
        return false;
    }

    let Some(details) = result.details.as_ref() else {
        return false;
    };
    details
        .get("execution_kind")
        .and_then(serde_json::Value::as_str)
        == Some("command")
}

async fn execute_tool_with_progress(
    call: tool_runtime::ToolCall,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
    context: &mut ToolCallExecutionContext<'_>,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) -> ToolResult {
    let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::unbounded_channel();
    let tool_context = ToolExecutionContext::new(context.cancellation)
        .with_permission_snapshot(permission_snapshot)
        .with_permission_handler(context.permission_handler.cloned())
        .with_progress_sink(ToolProgressSink::from_sender(progress_sender));
    let execution = context
        .executor
        .execute_tool_with_context(call, tool_context);
    tokio::pin!(execution);
    let mut progress_closed = false;

    let result = loop {
        tokio::select! {
            biased;
            maybe_progress = progress_receiver.recv(), if !progress_closed => {
                if let Some(progress) = maybe_progress {
                    emit_tool_progress(progress, context.clock, context.state, on_progress);
                } else {
                    progress_closed = true;
                };
            }
            result = &mut execution => break result,
        }
    };

    while let Ok(progress) = progress_receiver.try_recv() {
        emit_tool_progress(progress, context.clock, context.state, on_progress);
    }

    result
}

fn emit_tool_progress(
    progress: ToolProgress,
    clock: &ToolLoopClock,
    state: &mut RuntimeTurnState,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) {
    match progress {
        ToolProgress::SystemMessage { message } => {
            on_progress(ToolLoopProgress::SystemMessage { message });
        }
        ToolProgress::TerminalUpdated { snapshot } => {
            let snapshot = runtime_terminal_snapshot(snapshot);
            on_progress(ToolLoopProgress::TerminalUpdated {
                snapshot: snapshot.clone(),
            });
            state.observe_terminal_snapshot_output(&snapshot, clock.now(), on_progress);
        }
        ToolProgress::ManagedSearchToolAuthorization { tool_name } => {
            if let Some(tool) = ManagedSearchTool::from_binary_name(&tool_name) {
                on_progress(ToolLoopProgress::ManagedSearchToolAuthorization { tool });
            }
        }
    }
}

fn runtime_terminal_snapshot(snapshot: ToolTerminalSnapshot) -> RuntimeTerminalSnapshot {
    RuntimeTerminalSnapshot {
        terminal_id: snapshot.terminal_id,
        command: snapshot.command,
        cwd: snapshot.cwd,
        output: snapshot.output,
        truncated: snapshot.truncated,
        exit_status: snapshot.exit_status.map(runtime_terminal_exit_status),
        released: snapshot.released,
    }
}

fn runtime_terminal_exit_status(status: ToolTerminalExitStatus) -> RuntimeTerminalExitStatus {
    RuntimeTerminalExitStatus {
        exit_code: status.exit_code,
        signal: status.signal,
    }
}

async fn authorize_tool_call(
    call: &tool_runtime::ToolCall,
    context: &mut ToolCallExecutionContext<'_>,
) -> ToolAuthorization {
    let Some(definition) = context.tool_definitions.definition(&call.name).cloned() else {
        return ToolAuthorization::allow(None);
    };

    match definition.permission_policy {
        ToolPermissionPolicy::Always => ToolAuthorization::allow(None),
        ToolPermissionPolicy::Never => ToolAuthorization::deny(format!(
            "{TOOL_PERMISSION_DENIED}: {} is not allowed",
            definition.name
        )),
        ToolPermissionPolicy::Ask => {
            let Some(permission_handler) = context.permission_handler else {
                return ToolAuthorization::deny(format!(
                    "{TOOL_PERMISSION_DENIED}: {} requires approval",
                    definition.name
                ));
            };
            let mut permission_request = ToolPermissionRequest::new(call.clone(), definition);
            let preview =
                permission_preview_from_executor(context.executor, call, context.cancellation)
                    .await;
            let permission_snapshot = preview
                .as_ref()
                .and_then(|preview| preview.snapshot.clone());
            if let Some(preview) = preview {
                permission_request = permission_request.with_preview(preview);
            }
            match permission_handler
                .request_permission(permission_request, context.cancellation)
                .await
            {
                ToolPermissionDecision::Allow => ToolAuthorization::allow(permission_snapshot),
                ToolPermissionDecision::Deny { message } => ToolAuthorization::deny(message),
            }
        }
    }
}

async fn permission_preview_from_executor(
    executor: &ToolExecutorRegistry,
    call: &tool_runtime::ToolCall,
    cancellation: &CancellationToken,
) -> Option<ToolPermissionPreview> {
    if cancellation.is_cancelled() {
        return None;
    }
    let executor = executor.clone();
    let call = call.clone();
    let cancellation = cancellation.clone();
    tokio::task::spawn_blocking(move || executor.permission_preview(&call, &cancellation))
        .await
        .ok()
        .flatten()
}

struct ToolAuthorization {
    denial_message: Option<String>,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
}

impl ToolAuthorization {
    fn allow(permission_snapshot: Option<ToolPermissionFileSnapshot>) -> Self {
        Self {
            denial_message: None,
            permission_snapshot,
        }
    }

    fn deny(message: String) -> Self {
        Self {
            denial_message: Some(message),
            permission_snapshot: None,
        }
    }
}

pub(super) struct ToolCallExecutionContext<'a> {
    pub(super) executor: &'a ToolExecutorRegistry,
    pub(super) tool_definitions: &'a ToolRegistry,
    pub(super) cancellation: &'a CancellationToken,
    pub(super) clock: &'a ToolLoopClock,
    pub(super) permission_handler: Option<&'a SharedToolPermissionHandler>,
    pub(super) error_formatter: &'a SharedToolErrorFormatter,
    pub(super) state: &'a mut RuntimeTurnState,
}

pub(super) fn ai_tool_definitions_from_registry(registry: &ToolRegistry) -> Vec<AiToolDefinition> {
    registry
        .definitions()
        .map(|definition| {
            AiToolDefinition::new(
                definition.name.clone(),
                definition.description.clone().unwrap_or_default(),
                definition
                    .input_schema
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
            )
        })
        .collect()
}
