use mo_core::{
    acp::{AcpAgentIdentity, AcpPromptRequest},
    provider::ProviderKind,
    session::{
        NativeAgentRequest, RuntimeCapability, RuntimeCommand, RuntimeEvent, RuntimeIdentity,
        RuntimeModelConfig, RuntimeModelOption, RuntimePermissionOption,
        RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget,
        RuntimeTerminalExitStatus, RuntimeTerminalSnapshot, RuntimeToolActivity,
        RuntimeToolActivityContent, RuntimeToolActivityLocation, RuntimeToolActivityRawValue,
        RuntimeToolActivityStatus, RuntimeToolActivityUpdate, RuntimeToolKind,
    },
};

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
fn runtime_event_target_covers_rich_activity_surface() {
    let target = RuntimeTarget::acp_agent("kimi");
    let tool_activity = RuntimeToolActivity {
        activity_id: "tool-1".to_string(),
        title: "Read src/main.rs".to_string(),
        kind: RuntimeToolKind::Read,
        status: RuntimeToolActivityStatus::InProgress,
        content: vec![RuntimeToolActivityContent::Text("reading".to_string())],
        locations: vec![RuntimeToolActivityLocation {
            path: "src/main.rs".to_string(),
            line: Some(12),
        }],
        raw_input: Some(RuntimeToolActivityRawValue::from(
            serde_json::json!({ "path": "src/main.rs" }),
        )),
        raw_output: None,
    };
    let update = RuntimeToolActivityUpdate {
        activity_id: tool_activity.activity_id.clone(),
        status: Some(RuntimeToolActivityStatus::Completed),
        ..RuntimeToolActivityUpdate::default()
    };
    let terminal = RuntimeTerminalSnapshot {
        terminal_id: "term-1".to_string(),
        command: Some("cargo test".to_string()),
        cwd: Some("/repo".to_string()),
        output: "ok".to_string(),
        truncated: false,
        exit_status: Some(RuntimeTerminalExitStatus {
            exit_code: Some(0),
            signal: None,
        }),
        released: false,
    };
    let config = RuntimeModelConfig {
        config_id: Some("model".to_string()),
        current_value: "gpt-4.1".to_string(),
        current_name: "GPT 4.1".to_string(),
        options: vec![RuntimeModelOption {
            value: "gpt-4.1".to_string(),
            name: "GPT 4.1".to_string(),
        }],
    };

    let events = [
        RuntimeEvent::ToolActivityStarted {
            target: target.clone(),
            activity: tool_activity,
        },
        RuntimeEvent::ToolActivityUpdated {
            target: target.clone(),
            update,
        },
        RuntimeEvent::TerminalUpdated {
            target: target.clone(),
            snapshot: terminal,
        },
        RuntimeEvent::ModelConfigChanged {
            target: target.clone(),
            config,
        },
    ];

    for event in events {
        assert_eq!(event.target(), Some(&target));
    }
}

#[test]
fn runtime_permission_request_can_carry_tool_activity_preview() {
    let request = RuntimePermissionRequest::new(
        "permission-1",
        Some("Write file".to_string()),
        vec![RuntimePermissionOption::new(
            "allow-once",
            "Allow once",
            RuntimePermissionOptionKind::AllowOnce,
        )],
    )
    .with_tool_activity(RuntimeToolActivityUpdate {
        activity_id: "tool-write".to_string(),
        title: Some("WriteFile: src/lib.rs".to_string()),
        kind: Some(RuntimeToolKind::Edit),
        status: Some(RuntimeToolActivityStatus::Pending),
        content: Some(vec![RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: None,
            new_text: "pub fn added() {}".to_string(),
        }]),
        ..RuntimeToolActivityUpdate::default()
    });

    let activity = request
        .tool_activity
        .as_ref()
        .expect("runtime permission requests should expose previewable tool activity");
    assert_eq!(activity.activity_id, "tool-write");
    assert_eq!(
        activity
            .content
            .as_ref()
            .and_then(|content| content.first()),
        Some(&RuntimeToolActivityContent::Diff {
            path: "src/lib.rs".to_string(),
            old_text: None,
            new_text: "pub fn added() {}".to_string(),
        })
    );
}

#[test]
fn native_agent_capability_matches_current_tool_surface() {
    let capability = RuntimeCapability::agent();

    assert!(capability.supports_tools);
    assert!(!capability.supports_permissions);
    assert!(!capability.supports_model_config);
}

#[test]
fn runtime_request_policy_defaults_to_unbounded_tool_turns() {
    let policy = mo_core::request_policy::RuntimeRequestPolicy::default();

    assert_eq!(policy.tool_max_turns(), None);
}
