use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
use runtime_domain::{
    provider::ProviderKind,
    session::{
        ConversationResponse, ConversationTurnRequest, RuntimeCapability, RuntimeCommand,
        RuntimeEvent, RuntimeIdentity, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeTarget, RuntimeTerminalExitStatus,
        RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityLocation, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
        RuntimeToolActivityUpdate, RuntimeToolKind,
    },
};
use std::time::Duration;

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
    let target = RuntimeTarget::provider("openai", "gpt-4o-mini");
    let conversation_command =
        RuntimeCommand::submit_conversation_turn(ConversationTurnRequest::new(
            "openai",
            ProviderKind::OpenAi,
            "gpt-4o-mini",
            None,
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        ));
    let truncate_command = RuntimeCommand::truncate_conversation(1);
    let permission_command =
        RuntimeCommand::respond_permission(target.clone(), "permission-1", Some("allow".into()));
    let event = RuntimeEvent::Started {
        target: target.clone(),
        identity: RuntimeIdentity::new("gpt-4o-mini").with_source_label("openai"),
    };

    assert_eq!(conversation_command.target(), Some(&target));
    assert_eq!(truncate_command.target(), None);
    assert_eq!(permission_command.target(), Some(&target));
    assert_eq!(event.target(), Some(&target));
}

#[test]
fn conversation_response_carries_items_and_projects_visible_text() {
    let response = ConversationResponse {
        items: vec![
            ConversationItem::Reasoning {
                content: "think".to_string(),
                summary: None,
                encrypted: None,
            },
            ConversationItem::assistant_with_tool_calls(
                "checking".to_string(),
                vec![ToolCall::new("call-1", "read", "{}")],
            ),
            ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("tool output".to_string())],
                false,
            ),
            ConversationItem::text(Role::Assistant, "done"),
        ],
        reasoning_duration: Some(Duration::from_secs(2)),
    };

    assert_eq!(response.text_content(), "done");
    assert_eq!(response.reasoning_content().as_deref(), Some("think"));
    assert_eq!(response.items.len(), 4);
}

#[test]
fn runtime_event_target_covers_rich_activity_surface() {
    let target = RuntimeTarget::provider("openai", "gpt-4o-mini");
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
            is_truncated: false,
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
            is_truncated: false,
        })
    );
}

#[test]
fn runtime_capability_matches_current_tool_surface() {
    let capability = RuntimeCapability::conversation();

    assert!(capability.supports_tools);
    assert!(capability.supports_permissions);
    assert!(!capability.supports_model_config);
}

#[test]
fn runtime_request_policy_defaults_to_unbounded_tool_turns() {
    let policy = runtime_domain::request_policy::RuntimeRequestPolicy::default();

    assert_eq!(policy.tool_max_turns(), None);
}
