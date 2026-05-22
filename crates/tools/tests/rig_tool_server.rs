use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use mo_tools::{
    Tool, ToolCall, ToolDefinition, ToolExecutionFuture, ToolExecutorRegistry, ToolKind,
    ToolPermissionDecision, ToolPermissionFuture, ToolPermissionHandler, ToolPermissionPolicy,
    ToolPermissionRequest, ToolResult, rig::RigToolServer,
};
use tokio_util::sync::CancellationToken;

struct EchoTool;

struct EchoReplacementTool;

struct FailingTool;

struct DetailedTool;

struct AskTool {
    execution_count: Arc<AtomicUsize>,
}

struct FixedPermissionHandler {
    decision: ToolPermissionDecision,
}

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_description("Return the provided value")
            .with_permission_policy(ToolPermissionPolicy::Always)
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
            .with_permission_policy(ToolPermissionPolicy::Always)
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

impl Tool for FailingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("failing")
            .with_label("Failing")
            .with_kind(ToolKind::Other)
            .with_description("Return a tool error")
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::error(call.call_id, "raw failure (os error 2)") })
    }
}

impl Tool for DetailedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("detailed")
            .with_label("Detailed")
            .with_kind(ToolKind::Read)
            .with_description("Return output with result metadata")
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            let mut result = ToolResult::success(call.call_id, "line 10");
            result.details = Some(serde_json::json!({
                "kind": "text",
                "start_line": 10,
                "end_line": 10,
                "total_lines": 20,
                "next_offset": 11,
            }));
            result
        })
    }
}

impl Tool for AskTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("ask")
            .with_label("Ask")
            .with_kind(ToolKind::Other)
            .with_description("Return a value after approval")
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        let execution_count = Arc::clone(&self.execution_count);
        Box::pin(async move {
            execution_count.fetch_add(1, Ordering::SeqCst);
            ToolResult::success(call.call_id, "approved")
        })
    }
}

impl ToolPermissionHandler for FixedPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        assert_eq!(request.call.name, "ask");
        Box::pin(async move { self.decision.clone() })
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
async fn rig_tool_server_formats_tool_errors_as_clean_tool_results() {
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(FailingTool);

    let cancellation = CancellationToken::new();
    let server = RigToolServer::from_executor(executor, cancellation)
        .await
        .expect("rig tool server should build");

    let output = server
        .handle()
        .call_tool("failing", "{}")
        .await
        .expect("tool business errors should be returned as model-visible results");

    assert_eq!(
        output,
        "Tool failed: raw failure. Hint: Check the tool input and try again."
    );
    assert!(!output.contains("Toolset error"));
    assert!(!output.contains("ToolCallError"));
    assert!(!output.contains("os error"));
}

#[tokio::test]
async fn rig_tool_server_exposes_success_result_details_out_of_band() {
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(DetailedTool);

    let cancellation = CancellationToken::new();
    let server = RigToolServer::from_executor(executor, cancellation)
        .await
        .expect("rig tool server should build");
    let arguments = serde_json::json!({ "path": "Cargo.toml", "offset": 10 });

    let output = server
        .handle()
        .call_tool("detailed", &arguments.to_string())
        .await
        .expect("tool should execute");

    assert_eq!(output, "line 10");
    assert_eq!(
        server.take_tool_result_details("detailed", &arguments, &output),
        Some(serde_json::json!({
            "kind": "text",
            "start_line": 10,
            "end_line": 10,
            "total_lines": 20,
            "next_offset": 11,
        }))
    );
    assert_eq!(
        server.take_tool_result_details("detailed", &arguments, &output),
        None,
        "details should be consumed after the matching runtime update"
    );
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

#[tokio::test]
async fn rig_tool_server_ask_policy_requires_permission_handler_decision() {
    let execution_count = Arc::new(AtomicUsize::new(0));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(AskTool {
        execution_count: Arc::clone(&execution_count),
    });

    let cancellation = CancellationToken::new();
    let handler = Arc::new(FixedPermissionHandler {
        decision: ToolPermissionDecision::Deny {
            message: "approval rejected by test".to_string(),
        },
    });
    let server = RigToolServer::from_executor_with_permission_handler(
        executor,
        cancellation,
        Arc::new(mo_tools::DefaultToolErrorFormatter),
        handler,
    )
    .await
    .expect("rig tool server should build with permission handler");

    let output = server
        .handle()
        .call_tool("ask", "{}")
        .await
        .expect("permission denial should be returned as model-visible output");

    assert!(output.contains("approval rejected by test"));
    assert_eq!(
        execution_count.load(Ordering::SeqCst),
        0,
        "denied Ask tools must not execute"
    );
}

#[tokio::test]
async fn rig_tool_server_ask_policy_executes_after_permission_allow() {
    let execution_count = Arc::new(AtomicUsize::new(0));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(AskTool {
        execution_count: Arc::clone(&execution_count),
    });

    let cancellation = CancellationToken::new();
    let handler = Arc::new(FixedPermissionHandler {
        decision: ToolPermissionDecision::Allow,
    });
    let server = RigToolServer::from_executor_with_permission_handler(
        executor,
        cancellation,
        Arc::new(mo_tools::DefaultToolErrorFormatter),
        handler,
    )
    .await
    .expect("rig tool server should build with permission handler");

    let output = server
        .handle()
        .call_tool("ask", "{}")
        .await
        .expect("approved Ask tool should execute");

    assert_eq!(output, "approved");
    assert_eq!(execution_count.load(Ordering::SeqCst), 1);
}
