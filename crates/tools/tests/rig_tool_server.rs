use mo_tools::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolExecutorRegistry, ToolKind,
    ToolResult, rig::RigToolServer,
};
use tokio_util::sync::CancellationToken;

struct EchoTool;

struct EchoReplacementTool;

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
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

impl Tool for EchoReplacementTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo Replacement")
            .with_kind(ToolKind::Other)
            .with_description("Return the provided value with a replacement marker")
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
            ToolResult::success(call.call_id, format!("replacement:{value}"))
        })
    }
}

#[tokio::test]
async fn rig_tool_server_registers_executes_and_updates_tools_dynamically() {
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(EchoTool);

    let cancellation = CancellationToken::new();
    let mut server = RigToolServer::from_executor(executor, cancellation)
        .await
        .expect("rig tool server should build");

    let definitions = server
        .handle()
        .get_tool_defs(None)
        .await
        .expect("tool definitions should be available");
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "echo");

    let output = server
        .handle()
        .call_tool("echo", r#"{"value":"hello"}"#)
        .await
        .expect("registered tool should execute");
    assert_eq!(output, "hello");

    server
        .remove_tool("echo")
        .await
        .expect("existing tool should be removable");
    let definitions = server
        .handle()
        .get_tool_defs(None)
        .await
        .expect("tool definitions should still be available");
    assert!(
        definitions
            .iter()
            .all(|definition| definition.name != "echo")
    );

    server
        .add_tool(EchoTool)
        .await
        .expect("tool should be addable again");
    let output = server
        .handle()
        .call_tool("echo", r#"{"value":"again"}"#)
        .await
        .expect("re-added tool should execute");
    assert_eq!(output, "again");
}

#[tokio::test]
async fn rig_tool_server_replaces_tools_without_duplicate_definitions() {
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(EchoTool);

    let cancellation = CancellationToken::new();
    let mut server = RigToolServer::from_executor(executor, cancellation)
        .await
        .expect("rig tool server should build");

    server
        .add_tool(EchoReplacementTool)
        .await
        .expect("replacement tool should register");

    let definitions = server
        .handle()
        .get_tool_defs(None)
        .await
        .expect("tool definitions should be available");
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].name, "echo");
    assert_eq!(
        definitions[0].description,
        "Return the provided value with a replacement marker"
    );

    let output = server
        .handle()
        .call_tool("echo", r#"{"value":"hello"}"#)
        .await
        .expect("replacement tool should execute");
    assert_eq!(output, "replacement:hello");
}
