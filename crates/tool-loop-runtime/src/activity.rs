use provider_protocol::ToolCall as AiToolCall;
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityLocation,
    RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
    RuntimeToolKind,
};
use serde_json::Value;
use tool_runtime::{
    ProcessedToolError, ToolDefinition as LumosToolDefinition, ToolKind, ToolPermissionRequest,
    ToolRegistry, ToolResult,
};

/// `runtime_tool_activity_from_call` creates a TUI-visible activity from a provider tool call.
pub fn runtime_tool_activity_from_call(
    call: &AiToolCall,
    tool_definitions: &ToolRegistry,
) -> RuntimeToolActivity {
    let definition = tool_definitions.definition(&call.name);
    RuntimeToolActivity {
        activity_id: call.call_id.clone(),
        title: tool_title_for(&call.name, definition, &call.arguments),
        kind: runtime_kind_for(definition),
        status: RuntimeToolActivityStatus::InProgress,
        content: vec![RuntimeToolActivityContent::Text(tool_input_summary(
            &call.arguments,
        ))],
        locations: tool_locations_for(&call.arguments),
        raw_input: Some(RuntimeToolActivityRawValue::from(call.arguments.clone())),
        raw_output: None,
    }
}

/// `runtime_tool_activity_update_from_result` creates a final activity update for a tool result.
pub fn runtime_tool_activity_update_from_result(
    call: &AiToolCall,
    result: &ToolResult,
    processed_error: Option<&ProcessedToolError>,
    tool_definitions: &ToolRegistry,
) -> RuntimeToolActivityUpdate {
    let definition = tool_definitions.definition(&call.name);
    let status = Some(if processed_error.is_some() || result.is_error {
        RuntimeToolActivityStatus::Failed
    } else {
        RuntimeToolActivityStatus::Completed
    });
    let content = match processed_error {
        Some(processed) => {
            RuntimeToolActivityContent::Text(format!("Failed: {}", processed.display_reason))
        }
        None => RuntimeToolActivityContent::Text(result.content.clone()),
    };
    let raw_input = processed_error
        .is_none()
        .then(|| RuntimeToolActivityRawValue::from(call.arguments.clone()));
    let raw_output = processed_error.is_none().then(|| {
        RuntimeToolActivityRawValue::tool_result(result.content.clone(), result.details.clone())
    });

    RuntimeToolActivityUpdate {
        activity_id: call.call_id.clone(),
        title: Some(tool_title_for(&call.name, definition, &call.arguments)),
        kind: Some(runtime_kind_for(definition)),
        status,
        content: Some(vec![content]),
        locations: Some(tool_locations_for(&call.arguments)),
        raw_input,
        raw_output,
    }
}

/// `runtime_tool_activity_update_from_permission_request` previews an Ask permission request.
pub fn runtime_tool_activity_update_from_permission_request(
    activity_id: &str,
    request: &ToolPermissionRequest,
) -> RuntimeToolActivityUpdate {
    let tool_name = &request.call.name;
    let arguments = &request.call.arguments;
    RuntimeToolActivityUpdate {
        activity_id: activity_id.to_string(),
        title: Some(tool_title_for(
            tool_name,
            Some(&request.definition),
            arguments,
        )),
        kind: Some(runtime_kind_for(Some(&request.definition))),
        status: Some(RuntimeToolActivityStatus::Pending),
        content: Some(vec![RuntimeToolActivityContent::Text(tool_input_summary(
            arguments,
        ))]),
        locations: Some(tool_locations_for(arguments)),
        raw_input: Some(RuntimeToolActivityRawValue::from(arguments.clone())),
        raw_output: None,
    }
}

fn runtime_kind_for(definition: Option<&LumosToolDefinition>) -> RuntimeToolKind {
    match definition.map(|definition| definition.kind) {
        Some(ToolKind::Read) => RuntimeToolKind::Read,
        Some(ToolKind::Write) => RuntimeToolKind::Write,
        Some(ToolKind::Edit) => RuntimeToolKind::Edit,
        Some(ToolKind::Delete) => RuntimeToolKind::Delete,
        Some(ToolKind::Move) => RuntimeToolKind::Move,
        Some(ToolKind::Search) => RuntimeToolKind::Search,
        Some(ToolKind::Execute) => RuntimeToolKind::Execute,
        Some(ToolKind::Think) => RuntimeToolKind::Think,
        Some(ToolKind::Fetch) => RuntimeToolKind::Fetch,
        Some(ToolKind::SwitchMode) => RuntimeToolKind::SwitchMode,
        Some(ToolKind::Other) | None => RuntimeToolKind::Other,
    }
}

fn tool_title_for(
    tool_name: &str,
    definition: Option<&LumosToolDefinition>,
    arguments: &Value,
) -> String {
    let base = definition
        .and_then(|definition| definition.label.as_ref())
        .cloned()
        .unwrap_or_else(|| tool_name.to_string());

    if let Some(path) = arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        return format!("{base} {path}");
    }

    if let Some(command) = arguments
        .get("command")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        return format!("{base} {command}");
    }

    base
}

fn tool_locations_for(arguments: &Value) -> Vec<RuntimeToolActivityLocation> {
    arguments
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| {
            vec![RuntimeToolActivityLocation {
                path: path.to_string(),
                line: None,
            }]
        })
        .unwrap_or_default()
}

fn tool_input_summary(arguments: &Value) -> String {
    if arguments.is_null() {
        return "{}".to_string();
    }

    serde_json::to_string_pretty(arguments).unwrap_or_else(|_| arguments.to_string())
}

#[cfg(test)]
mod tests {
    use provider_protocol::ToolCall;
    use runtime_domain::session::{RuntimeToolActivityContent, RuntimeToolActivityStatus};
    use tool_runtime::{ToolDefinition, ToolKind, ToolRegistry, ToolResult};

    use super::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result};

    #[test]
    fn runtime_tool_activity_titles_include_paths() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("read")
                .with_label("Read")
                .with_kind(ToolKind::Read),
        );
        let call = ToolCall::new(
            "call-1",
            "read",
            serde_json::json!({ "path": "Cargo.toml" }),
        );

        let activity = runtime_tool_activity_from_call(&call, &registry);

        assert_eq!(activity.activity_id, "call-1");
        assert_eq!(activity.title, "Read Cargo.toml");
    }

    #[test]
    fn successful_result_preserves_tool_details() {
        let mut registry = ToolRegistry::new();
        registry.insert(ToolDefinition::new("read").with_label("Read"));
        let call = ToolCall::new(
            "call-1",
            "read",
            serde_json::json!({ "path": "Cargo.toml" }),
        );
        let mut result = ToolResult::success("call-1", "content");
        result.details = Some(serde_json::json!({ "kind": "text" }));

        let update = runtime_tool_activity_update_from_result(&call, &result, None, &registry);

        assert!(matches!(
            update.status,
            Some(RuntimeToolActivityStatus::Completed)
        ));
        assert!(matches!(
            update.content.as_deref(),
            Some([RuntimeToolActivityContent::Text(text)]) if text == "content"
        ));
        let raw_output = update.raw_output.expect("raw output should be preserved");
        assert_eq!(
            raw_output.tool_result_details(),
            Some(&serde_json::json!({ "kind": "text" }))
        );
    }
}
