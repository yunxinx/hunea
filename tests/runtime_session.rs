use lumos::runtime::session::{
    RuntimeCommand, RuntimeEvent, RuntimeIdentity, RuntimePermissionOption,
    RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget,
};
use lumos::runtime::tools::{RuntimeToolDefinition, ToolPermissionPolicy};

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
    let target = RuntimeTarget::native_chat("openai", "gpt-4o-mini");
    let command = RuntimeCommand::submit_prompt(target.clone(), "hello");
    let permission_command =
        RuntimeCommand::respond_permission(target.clone(), "permission-1", Some("allow".into()));
    let config_command = RuntimeCommand::set_config_option(target.clone(), "model", "gpt-4.1-mini");
    let event = RuntimeEvent::Started {
        target: target.clone(),
        identity: RuntimeIdentity::new("gpt-4o-mini").with_source_label("openai"),
    };

    assert_eq!(command.target(), Some(&target));
    assert_eq!(permission_command.target(), Some(&target));
    assert_eq!(config_command.target(), Some(&target));
    assert_eq!(event.target(), Some(&target));
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
