use mo_core::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityLocation,
    RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
    RuntimeToolKind,
};
use mo_tools::{ToolDefinition as LumosToolDefinition, ToolKind, ToolRegistry};
use rig_core::{
    OneOrMany,
    message::{ToolCall as RigToolCall, ToolResultContent},
};
use serde_json::Value;

use super::tool_errors::parse_rig_error_text;

pub(crate) fn runtime_tool_activity_from_rig_call(
    tool_call: &RigToolCall,
    internal_call_id: &str,
    tool_definitions: &ToolRegistry,
) -> RuntimeToolActivity {
    let tool_name = &tool_call.function.name;
    let definition = tool_definitions.definition(tool_name);
    let kind = runtime_kind_for(definition);
    let title = tool_title_for(tool_name, definition, &tool_call.function.arguments);
    let locations = tool_locations_for(&tool_call.function.arguments);

    RuntimeToolActivity {
        activity_id: internal_call_id.to_string(),
        title,
        kind,
        status: RuntimeToolActivityStatus::InProgress,
        content: vec![RuntimeToolActivityContent::Text(tool_input_summary(
            &tool_call.function.arguments,
        ))],
        locations,
        raw_input: Some(RuntimeToolActivityRawValue::from(
            tool_call.function.arguments.clone(),
        )),
        raw_output: None,
    }
}

pub(crate) fn runtime_tool_activity_update_from_rig_result(
    internal_call_id: &str,
    tool_call: &RigToolCall,
    result: &str,
    tool_definitions: &ToolRegistry,
) -> RuntimeToolActivityUpdate {
    let tool_name = &tool_call.function.name;
    let definition = tool_definitions.definition(tool_name);
    let kind = Some(runtime_kind_for(definition));
    let title = Some(tool_title_for(
        tool_name,
        definition,
        &tool_call.function.arguments,
    ));
    let error_reason = parse_rig_error_text(result);
    let status = Some(if error_reason.is_some() {
        RuntimeToolActivityStatus::Failed
    } else {
        RuntimeToolActivityStatus::Completed
    });
    let content = match error_reason.as_ref() {
        Some(reason) => RuntimeToolActivityContent::Text(format!("Failed: {reason}")),
        None => RuntimeToolActivityContent::Text(result.to_string()),
    };
    let raw_input = error_reason
        .is_none()
        .then(|| RuntimeToolActivityRawValue::from(tool_call.function.arguments.clone()));
    let raw_output = error_reason
        .is_none()
        .then(|| RuntimeToolActivityRawValue::from(result));

    RuntimeToolActivityUpdate {
        activity_id: internal_call_id.to_string(),
        title,
        kind,
        status,
        content: Some(vec![content]),
        locations: Some(tool_locations_for(&tool_call.function.arguments)),
        raw_input,
        raw_output,
    }
}

pub(crate) fn tool_result_text(content: &OneOrMany<ToolResultContent>) -> String {
    content
        .iter()
        .map(|content| match content {
            ToolResultContent::Text(text) => text.text.as_str(),
            ToolResultContent::Image(_) => "[image]",
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn runtime_kind_for(definition: Option<&LumosToolDefinition>) -> RuntimeToolKind {
    match definition.map(|definition| definition.kind) {
        Some(ToolKind::Read) => RuntimeToolKind::Read,
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
    use mo_core::session::{
        RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
    };
    use mo_tools::{ToolDefinition, ToolKind, ToolRegistry};
    use rig_core::message::{ToolCall as RigToolCall, ToolFunction};

    use super::{
        runtime_tool_activity_from_rig_call, runtime_tool_activity_update_from_rig_result,
    };

    #[test]
    fn runtime_tool_activity_titles_include_paths() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("file_read")
                .with_label("Read")
                .with_kind(ToolKind::Read),
        );
        let tool_call = RigToolCall {
            id: "rig-call".to_string(),
            call_id: Some("call-1".to_string()),
            function: ToolFunction::new(
                "file_read".to_string(),
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
            signature: None,
            additional_params: None,
        };

        let activity = runtime_tool_activity_from_rig_call(&tool_call, "internal-1", &registry);
        assert_eq!(activity.activity_id, "internal-1");
        assert_eq!(activity.title, "Read Cargo.toml");
        assert_eq!(activity.kind, RuntimeToolKind::Read);
        assert!(matches!(
            activity.status,
            RuntimeToolActivityStatus::InProgress
        ));
        assert!(activity.raw_input.is_some());
    }

    #[test]
    fn runtime_tool_activity_update_marks_tool_errors_failed() {
        let mut registry = ToolRegistry::new();
        registry.insert(ToolDefinition::new("file_read").with_label("Read"));
        let tool_call = RigToolCall {
            id: "rig-call".to_string(),
            call_id: Some("call-1".to_string()),
            function: ToolFunction::new(
                "file_read".to_string(),
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
            signature: None,
            additional_params: None,
        };

        let update = runtime_tool_activity_update_from_rig_result(
            "internal-1",
            &tool_call,
            "Toolset error: ToolCallError: not found",
            &registry,
        );

        assert_eq!(update.activity_id, "internal-1");
        assert_eq!(update.status, Some(RuntimeToolActivityStatus::Failed));
        assert_eq!(
            update.content,
            Some(vec![RuntimeToolActivityContent::Text(
                "Failed: not found".to_string()
            )])
        );
        assert!(update.raw_input.is_none());
        assert!(update.raw_output.is_none());
    }

    #[test]
    fn runtime_tool_activity_update_marks_formatted_tool_errors_failed() {
        let mut registry = ToolRegistry::new();
        registry.insert(ToolDefinition::new("file_read").with_label("Read"));
        let tool_call = RigToolCall {
            id: "rig-call".to_string(),
            call_id: Some("call-1".to_string()),
            function: ToolFunction::new(
                "file_read".to_string(),
                serde_json::json!({ "path": "AGENTS.md" }),
            ),
            signature: None,
            additional_params: None,
        };

        let update = runtime_tool_activity_update_from_rig_result(
            "internal-1",
            &tool_call,
            "File not found: AGENTS.md. Hint: Use list_dir to verify the file exists before reading.",
            &registry,
        );

        assert_eq!(update.status, Some(RuntimeToolActivityStatus::Failed));
        assert_eq!(
            update.content,
            Some(vec![RuntimeToolActivityContent::Text(
                "Failed: File not found: AGENTS.md".to_string()
            )])
        );
        assert!(update.raw_input.is_none());
        assert!(update.raw_output.is_none());
    }

    #[test]
    fn runtime_tool_activity_uses_definition_kind_instead_of_name_aliases() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("workspace_read")
                .with_label("Read")
                .with_kind(ToolKind::Read),
        );
        let tool_call = RigToolCall {
            id: "rig-call".to_string(),
            call_id: Some("call-1".to_string()),
            function: ToolFunction::new(
                "workspace_read".to_string(),
                serde_json::json!({ "path": "Cargo.toml" }),
            ),
            signature: None,
            additional_params: None,
        };

        let activity = runtime_tool_activity_from_rig_call(&tool_call, "internal-1", &registry);

        assert_eq!(activity.kind, RuntimeToolKind::Read);
    }

    #[test]
    fn runtime_tool_activity_does_not_infer_kind_from_unregistered_tool_names() {
        let registry = ToolRegistry::new();
        for tool_name in ["read_file", "shell", "exec", "execute"] {
            let tool_call = RigToolCall {
                id: format!("rig-call-{tool_name}"),
                call_id: Some(format!("call-{tool_name}")),
                function: ToolFunction::new(
                    tool_name.to_string(),
                    serde_json::json!({ "path": "Cargo.toml", "command": "pwd" }),
                ),
                signature: None,
                additional_params: None,
            };

            let activity = runtime_tool_activity_from_rig_call(&tool_call, "internal-1", &registry);

            assert_eq!(activity.kind, RuntimeToolKind::Other, "{tool_name}");
        }
    }
}
