use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolExecutor, ToolExecutorRegistry,
    ToolKind, ToolPermissionPolicy, ToolResult,
};

#[test]
fn tool_definition_keeps_schema_and_permission_policy() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string" }
        },
        "required": ["command"]
    });

    let definition = ToolDefinition::new("shell")
        .with_label("Shell")
        .with_kind(ToolKind::Execute)
        .with_description("Run a shell command")
        .with_input_schema(schema.clone())
        .with_permission_policy(ToolPermissionPolicy::Ask);

    assert_eq!(definition.name, "shell");
    assert_eq!(definition.label.as_deref(), Some("Shell"));
    assert_eq!(definition.kind, ToolKind::Execute);
    assert_eq!(definition.input_schema.as_ref(), Some(&schema));
    assert_eq!(definition.permission_policy, ToolPermissionPolicy::Ask);
}

struct EchoTool;

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_description("Return the provided value")
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            let value = call
                .arguments
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            ToolResult::success(call.call_id, value)
        })
    }
}

#[tokio::test]
async fn tool_executor_registry_exposes_definitions_and_executes_by_name() {
    let mut registry = ToolExecutorRegistry::new();
    registry.insert(EchoTool);

    let definitions = registry.definitions();
    let echo = definitions
        .definition("echo")
        .expect("registered tool should expose its definition");
    assert_eq!(echo.label.as_deref(), Some("Echo"));

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "echo", serde_json::json!({ "value": "hello" })),
            &CancellationToken::new(),
        )
        .await;

    assert_eq!(result, ToolResult::success("call-1", "hello"));
}

#[tokio::test]
async fn tool_executor_registry_returns_model_visible_unknown_tool_error() {
    let registry = ToolExecutorRegistry::new();

    let result = registry
        .execute_tool(
            ToolCall::new("call-1", "missing", serde_json::json!({})),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.call_id, "call-1");
    assert!(result.content.contains("missing"));
}
