use mo_core::tools::{
    RuntimeTool, RuntimeToolCall, RuntimeToolDefinition, RuntimeToolExecutionFuture,
    RuntimeToolExecutor, RuntimeToolExecutorRegistry, RuntimeToolResult, ToolPermissionPolicy,
};
use mo_core::{
    acp::{AcpAgentIdentity, AcpPromptRequest},
    provider::ProviderKind,
    session::{
        NativeAgentRequest, RuntimeCapability, RuntimeCommand, RuntimeEvent, RuntimeIdentity,
        RuntimePermissionOption, RuntimePermissionOptionKind, RuntimePermissionRequest,
        RuntimeTarget,
    },
};
use tokio_util::sync::CancellationToken;

#[test]
fn runtime_permission_request_selects_cancel_reject_fallback() {
    let request = RuntimePermissionRequest::new(
        "permission-1",
        Some("Run shell command".to_string()),
        vec![RuntimePermissionOption::new(
            "reject-session",
            "Reject in session",
            RuntimePermissionOptionKind::RejectAlways,
        )],
    );

    assert_eq!(
        request.reject_for_cancel(),
        Some("reject-session".to_string())
    );
}

#[test]
fn runtime_command_and_event_carry_target_identity() {
    let target = RuntimeTarget::native_agent("openai", "gpt-4o-mini");
    let command = RuntimeCommand::submit_prompt(target.clone(), "hello");
    let native_command = RuntimeCommand::submit_native_agent(NativeAgentRequest::new(
        "openai",
        ProviderKind::OpenAi,
        "gpt-4o-mini",
        None,
        None,
        None,
        Vec::new(),
    ));
    let acp_target = RuntimeTarget::acp_agent("kimi");
    let acp_command = RuntimeCommand::submit_acp_prompt(AcpPromptRequest {
        agent_id: "kimi".to_string(),
        text: "hello".to_string(),
        current_dir: std::path::PathBuf::from("."),
        identity: Box::<AcpAgentIdentity>::default(),
    });
    let permission_command =
        RuntimeCommand::respond_permission(target.clone(), "permission-1", Some("allow".into()));
    let config_command = RuntimeCommand::set_config_option(target.clone(), "model", "gpt-4.1-mini");
    let event = RuntimeEvent::Started {
        target: target.clone(),
        identity: RuntimeIdentity::new("gpt-4o-mini").with_source_label("openai"),
    };

    assert_eq!(command.target(), Some(&target));
    assert_eq!(native_command.target(), Some(&target));
    assert_eq!(acp_command.target(), Some(&acp_target));
    assert_eq!(permission_command.target(), Some(&target));
    assert_eq!(config_command.target(), Some(&target));
    assert_eq!(event.target(), Some(&target));
}

#[test]
fn native_agent_capability_matches_current_tool_surface() {
    let capability = RuntimeCapability::agent();

    assert!(capability.supports_tools);
    assert!(!capability.supports_permissions);
    assert!(!capability.supports_model_config);
}

#[test]
fn runtime_tool_definition_keeps_schema_and_permission_policy() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "command": { "type": "string" }
        },
        "required": ["command"]
    });

    let definition = RuntimeToolDefinition::new("shell")
        .with_label("Shell")
        .with_description("Run a shell command")
        .with_input_schema(schema.clone())
        .with_permission_policy(ToolPermissionPolicy::Ask);

    assert_eq!(definition.name, "shell");
    assert_eq!(definition.label.as_deref(), Some("Shell"));
    assert_eq!(definition.input_schema.as_ref(), Some(&schema));
    assert_eq!(definition.permission_policy, ToolPermissionPolicy::Ask);
}

struct EchoRuntimeTool;

impl RuntimeTool for EchoRuntimeTool {
    fn definition(&self) -> RuntimeToolDefinition {
        RuntimeToolDefinition::new("echo")
            .with_label("Echo")
            .with_description("Return the provided value")
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> RuntimeToolExecutionFuture<'a> {
        Box::pin(async move {
            let value = call
                .arguments
                .get("value")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            RuntimeToolResult::success(call.call_id, value)
        })
    }
}

#[tokio::test]
async fn runtime_tool_executor_registry_exposes_definitions_and_executes_by_name() {
    let mut registry = RuntimeToolExecutorRegistry::new();
    registry.insert(EchoRuntimeTool);

    let definitions = registry.definitions();
    let echo = definitions
        .definition("echo")
        .expect("registered tool should expose its definition");
    assert_eq!(echo.label.as_deref(), Some("Echo"));

    let result = registry
        .execute_tool(
            RuntimeToolCall::new("call-1", "echo", serde_json::json!({ "value": "hello" })),
            &CancellationToken::new(),
        )
        .await;

    assert_eq!(result, RuntimeToolResult::success("call-1", "hello"));
}

#[tokio::test]
async fn runtime_tool_executor_registry_returns_model_visible_unknown_tool_error() {
    let registry = RuntimeToolExecutorRegistry::new();

    let result = registry
        .execute_tool(
            RuntimeToolCall::new("call-1", "missing", serde_json::json!({})),
            &CancellationToken::new(),
        )
        .await;

    assert!(result.is_error);
    assert_eq!(result.call_id, "call-1");
    assert!(result.content.contains("missing"));
}
