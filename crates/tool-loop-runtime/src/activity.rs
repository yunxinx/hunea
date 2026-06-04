use provider_protocol::{ToolCall as AiToolCall, ToolCallArgumentsError};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityLocation,
    RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolActivityUpdate,
    RuntimeToolKind,
};
use serde_json::Value;
use tool_runtime::{
    ProcessedToolError, ToolDefinition as HuneaToolDefinition, ToolKind, ToolPermissionRequest,
    ToolRegistry, ToolResult,
};

/// `runtime_tool_activity_from_call` creates a TUI-visible activity from a provider tool call.
pub fn runtime_tool_activity_from_call(
    call: &AiToolCall,
    tool_definitions: &ToolRegistry,
) -> RuntimeToolActivity {
    let definition = tool_definitions.definition(&call.name);
    let parsed = ParsedArguments::from_call(call);
    let arguments = parsed.value();
    let content = if runtime_kind_for(definition) == RuntimeToolKind::Execute {
        vec![RuntimeToolActivityContent::Terminal {
            terminal_id: call.call_id.clone(),
        }]
    } else {
        vec![RuntimeToolActivityContent::Text(parsed.input_summary())]
    };

    RuntimeToolActivity {
        activity_id: call.call_id.clone(),
        title: tool_title_for(&call.name, definition, arguments),
        kind: runtime_kind_for(definition),
        status: RuntimeToolActivityStatus::InProgress,
        content,
        locations: tool_locations_for(arguments),
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
    let parsed = ParsedArguments::from_call(call);
    let arguments = parsed.value();
    let status = Some(if processed_error.is_some() || result.is_error {
        RuntimeToolActivityStatus::Failed
    } else {
        RuntimeToolActivityStatus::Completed
    });
    let content = match processed_error {
        Some(processed) => {
            RuntimeToolActivityContent::Text(format!("Failed: {}", processed.display_reason))
        }
        None => runtime_tool_activity_content_for_result(arguments, result, definition),
    };
    let raw_input = processed_error
        .is_none()
        .then(|| RuntimeToolActivityRawValue::from(call.arguments.clone()));
    let raw_output = processed_error.is_none().then(|| {
        RuntimeToolActivityRawValue::tool_result_with_display_content(
            result.content.clone(),
            result.display_content.clone(),
            result.details.clone(),
        )
    });

    RuntimeToolActivityUpdate {
        activity_id: call.call_id.clone(),
        title: Some(tool_title_for(&call.name, definition, arguments)),
        kind: Some(runtime_kind_for(definition)),
        status,
        content: Some(vec![content]),
        locations: Some(tool_locations_for(arguments)),
        raw_input,
        raw_output,
    }
}

fn runtime_tool_activity_content_for_result(
    arguments: &Value,
    result: &ToolResult,
    definition: Option<&HuneaToolDefinition>,
) -> RuntimeToolActivityContent {
    if matches!(
        runtime_kind_for(definition),
        RuntimeToolKind::Edit | RuntimeToolKind::Write
    ) && let Some(content) = diff_content_from_tool_result(arguments, result)
    {
        return content;
    }

    RuntimeToolActivityContent::Text(
        result
            .display_content
            .as_ref()
            .unwrap_or(&result.content)
            .clone(),
    )
}

fn diff_content_from_tool_result(
    arguments: &Value,
    result: &ToolResult,
) -> Option<RuntimeToolActivityContent> {
    let details = result.details.as_ref()?;
    let path = details
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| arguments.get("path").and_then(Value::as_str))?
        .to_string();
    let new_text = details.get("new_text")?.as_str()?.to_string();
    let old_text = details
        .get("old_text")
        .and_then(Value::as_str)
        .map(str::to_string);
    let is_truncated = details
        .get("preview_truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Some(RuntimeToolActivityContent::Diff {
        path,
        old_text,
        new_text,
        is_truncated,
    })
}

/// `runtime_tool_activity_update_from_permission_request` previews an Ask permission request.
pub fn runtime_tool_activity_update_from_permission_request(
    activity_id: &str,
    request: &ToolPermissionRequest,
) -> RuntimeToolActivityUpdate {
    let tool_name = &request.call.name;
    let arguments = &request.call.arguments;
    let content = request.preview.as_ref().map_or_else(
        || RuntimeToolActivityContent::Text(tool_input_summary(arguments)),
        |preview| RuntimeToolActivityContent::Diff {
            path: preview.path.clone(),
            old_text: preview.old_text.clone(),
            new_text: preview.new_text.clone(),
            is_truncated: preview.is_truncated,
        },
    );
    RuntimeToolActivityUpdate {
        activity_id: activity_id.to_string(),
        title: Some(tool_title_for(
            tool_name,
            Some(&request.definition),
            arguments,
        )),
        kind: Some(runtime_kind_for(Some(&request.definition))),
        status: Some(RuntimeToolActivityStatus::Pending),
        content: Some(vec![content]),
        locations: Some(tool_locations_for(arguments)),
        raw_input: Some(RuntimeToolActivityRawValue::from(arguments.clone())),
        raw_output: None,
    }
}

fn runtime_kind_for(definition: Option<&HuneaToolDefinition>) -> RuntimeToolKind {
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
    definition: Option<&HuneaToolDefinition>,
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

/// 解析后的 tool call arguments，区分合法 JSON 与解析失败。
///
/// 解析失败时保留原始字符串，展示层可显示 raw arguments 或 invalid JSON 标记，
/// 而不是静默降级为空参数。
enum ParsedArguments {
    Valid(Value),
    Invalid {
        raw: String,
        error: ToolCallArgumentsError,
    },
}

impl ParsedArguments {
    /// 从 provider tool call 解析 arguments。
    fn from_call(call: &AiToolCall) -> Self {
        match call.parsed_arguments_value() {
            Ok(value) => Self::Valid(value),
            Err(error) => Self::Invalid {
                raw: call.arguments.clone(),
                error,
            },
        }
    }

    /// 返回解析后的 JSON value 引用；解析失败时返回空对象。
    fn value(&self) -> &Value {
        match self {
            Self::Valid(value) => value,
            Self::Invalid { .. } => &EMPTY_OBJECT,
        }
    }

    /// 返回展示用的输入摘要：合法 JSON 格式化输出，失败时标记原始内容。
    fn input_summary(&self) -> String {
        match self {
            Self::Valid(value) => tool_input_summary(value),
            Self::Invalid { raw, error } => format!("[invalid JSON] {error}; raw: {raw}"),
        }
    }
}

static EMPTY_OBJECT: std::sync::LazyLock<Value> =
    std::sync::LazyLock::new(|| Value::Object(serde_json::Map::new()));

#[cfg(test)]
mod tests {
    use provider_protocol::ToolCall;
    use runtime_domain::session::{
        RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
    };
    use tool_runtime::{
        ToolDefinition, ToolKind, ToolPermissionPreview, ToolPermissionRequest, ToolRegistry,
        ToolResult,
    };

    use super::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result};

    #[test]
    fn runtime_tool_activity_titles_include_paths() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("read")
                .with_label("Read")
                .with_kind(ToolKind::Read),
        );
        let call = ToolCall::new("call-1", "read", r#"{"path":"Cargo.toml"}"#);

        let activity = runtime_tool_activity_from_call(&call, &registry);

        assert_eq!(activity.activity_id, "call-1");
        assert_eq!(activity.title, "Read Cargo.toml");
    }

    #[test]
    fn successful_result_preserves_tool_details() {
        let mut registry = ToolRegistry::new();
        registry.insert(ToolDefinition::new("read").with_label("Read"));
        let call = ToolCall::new("call-1", "read", r#"{"path":"Cargo.toml"}"#);
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

    #[test]
    fn edit_result_with_details_becomes_diff_content() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("edit")
                .with_label("Edit")
                .with_kind(ToolKind::Edit),
        );
        let call = ToolCall::new(
            "call-1",
            "edit",
            r#"{"path":"test/temp.md","edits":[{"old_string":"old\n","new_string":"new\n"}]}"#
                .to_string(),
        );
        let result = ToolResult::success(
            "call-1",
            "Successfully replaced 1 block(s) in test/temp.md.",
        )
        .with_details(serde_json::json!({
            "path": "test/temp.md",
            "old_text": "old\n",
            "new_text": "new\n",
            "replacements": 1
        }));

        let update = runtime_tool_activity_update_from_result(&call, &result, None, &registry);

        assert!(matches!(
            update.content.as_deref(),
            Some([RuntimeToolActivityContent::Diff {
                path,
                old_text,
                new_text,
                ..
            }]) if path == "test/temp.md"
                && old_text.as_deref() == Some("old\n")
                && new_text == "new\n"
        ));
        assert_eq!(update.title.as_deref(), Some("Edit test/temp.md"));
        assert_eq!(update.kind, Some(RuntimeToolKind::Edit));
        let raw_output = update.raw_output.expect("raw output should be preserved");
        assert_eq!(
            raw_output.tool_result_details(),
            Some(&serde_json::json!({
                "path": "test/temp.md",
                "old_text": "old\n",
                "new_text": "new\n",
                "replacements": 1
            }))
        );
    }

    #[test]
    fn write_result_with_details_becomes_diff_content() {
        let mut registry = ToolRegistry::new();
        registry.insert(
            ToolDefinition::new("write")
                .with_label("Write")
                .with_kind(ToolKind::Write),
        );
        let call = ToolCall::new(
            "call-1",
            "write",
            r#"{"path":"test/temp.md","content":"new\n"}"#.to_string(),
        );
        let result = ToolResult::success(
            "call-1",
            "The file test/temp.md has been updated successfully.",
        )
        .with_details(serde_json::json!({
            "path": "test/temp.md",
            "old_text": "old\n",
            "new_text": "new\n"
        }));

        let update = runtime_tool_activity_update_from_result(&call, &result, None, &registry);

        assert!(matches!(
            update.content.as_deref(),
            Some([RuntimeToolActivityContent::Diff {
                path,
                old_text,
                new_text,
                ..
            }]) if path == "test/temp.md"
                && old_text.as_deref() == Some("old\n")
                && new_text == "new\n"
        ));
        assert_eq!(update.title.as_deref(), Some("Write test/temp.md"));
        assert_eq!(update.kind, Some(RuntimeToolKind::Write));
    }

    #[test]
    fn permission_request_preview_becomes_diff_content() {
        let definition = ToolDefinition::new("edit")
            .with_label("Edit")
            .with_kind(ToolKind::Edit);
        let request = ToolPermissionRequest::new(
            tool_runtime::ToolCall::new(
                "call-1",
                "edit",
                serde_json::json!({
                    "path": "temp.md",
                    "edits": [
                        { "old_string": "old\n", "new_string": "new\n" }
                    ]
                }),
            ),
            definition,
        )
        .with_preview(ToolPermissionPreview {
            path: "temp.md".to_string(),
            old_text: Some("old\n".to_string()),
            new_text: "new\n".to_string(),
            is_truncated: false,
            snapshot: None,
        });

        let update =
            super::runtime_tool_activity_update_from_permission_request("call-1", &request);

        assert!(matches!(
            update.content.as_deref(),
            Some([RuntimeToolActivityContent::Diff {
                path,
                old_text,
                new_text,
                ..
            }]) if path == "temp.md"
                && old_text.as_deref() == Some("old\n")
                && new_text == "new\n"
        ));
    }

    #[test]
    fn invalid_json_arguments_shows_raw_content_in_activity() {
        let registry = ToolRegistry::new();
        let call = ToolCall::new("call-1", "unknown_tool", "not valid json");

        let activity = super::runtime_tool_activity_from_call(&call, &registry);

        match &activity.content[..] {
            [RuntimeToolActivityContent::Text(summary)] => {
                assert!(
                    summary.contains("[invalid JSON]"),
                    "expected invalid JSON marker, got: {summary}"
                );
                assert!(
                    summary.contains("not valid json"),
                    "expected raw arguments preserved, got: {summary}"
                );
            }
            other => panic!("expected Text content, got: {other:?}"),
        }
    }

    #[test]
    fn invalid_json_arguments_preserve_raw_input() {
        let registry = ToolRegistry::new();
        let call = ToolCall::new("call-1", "unknown_tool", "not valid json");

        let activity = super::runtime_tool_activity_from_call(&call, &registry);
        assert_eq!(
            activity
                .raw_input
                .as_ref()
                .and_then(|raw| raw.display_text())
                .as_deref(),
            Some("not valid json")
        );

        let result = ToolResult::error("call-1", "Invalid tool call arguments");
        let update =
            super::runtime_tool_activity_update_from_result(&call, &result, None, &registry);
        assert_eq!(
            update
                .raw_input
                .as_ref()
                .and_then(|raw| raw.display_text())
                .as_deref(),
            Some("not valid json")
        );
    }

    #[test]
    fn empty_arguments_treated_as_empty_object() {
        let call = ToolCall::new("call-1", "unknown_tool", String::new());
        let parsed = super::ParsedArguments::from_call(&call);
        assert!(parsed.value().is_object());
        assert_eq!(parsed.input_summary(), "{}");
    }

    #[test]
    fn valid_json_arguments_parse_correctly() {
        let call = ToolCall::new(
            "call-1",
            "unknown_tool",
            r#"{"path":"Cargo.toml"}"#.to_string(),
        );
        let parsed = super::ParsedArguments::from_call(&call);
        assert!(matches!(parsed.value(), serde_json::Value::Object(_)));
        assert!(parsed.input_summary().contains("Cargo.toml"));
    }
}
