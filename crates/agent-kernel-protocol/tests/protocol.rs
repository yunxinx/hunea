use agent_kernel_protocol::*;
use provider_protocol::{ContentBlock, ConversationItem, Role};

fn target() -> AgentKernelTarget {
    AgentKernelTarget {
        provider_id: "local".to_string(),
        model_id: "model".to_string(),
    }
}

fn turn_request() -> AgentKernelTurnRequest {
    AgentKernelTurnRequest {
        target: target(),
        delivery: AgentKernelUserDelivery {
            content: "visible delivery sentinel".to_string(),
            attachments: vec![AgentKernelUserAttachment::Image {
                data_base64: "private image bytes".to_string(),
                mime_type: "image/png".to_string(),
                uri: Some("/private/image.png".to_string()),
                detail: Some(AgentKernelImageDetail::Original),
            }],
        },
        controls: AgentKernelTurnControls {
            skill_bindings: vec![AgentKernelSkillBinding {
                skill_name: "private-skill".to_string(),
                origin: AgentKernelPromptOrigin::Project,
                skill_path: "/private/SKILL.md".to_string(),
                start_char: 0,
                end_char: 14,
            }],
            custom_prompt_bindings: vec![AgentKernelCustomPromptBinding {
                reference_id: "private-prompt".to_string(),
                origin: AgentKernelPromptOrigin::Global,
                start_char: 15,
                end_char: 30,
            }],
        },
        provider_content: vec![ContentBlock::Text(
            "provider-visible content sentinel".to_string(),
        )],
    }
}

fn turn_event(kind: AgentKernelEventKind) -> AgentKernelEvent {
    AgentKernelEvent {
        agent_id: 1,
        turn_id: 7,
        target: target(),
        kind,
    }
}

#[test]
fn methods_and_envelopes_round_trip_with_stable_wire_names() {
    for (method, expected) in [
        (AgentKernelMethod::Initialize, "initialize"),
        (AgentKernelMethod::AgentCommand, "agent.command"),
        (AgentKernelMethod::Shutdown, "shutdown"),
    ] {
        assert_eq!(method.as_str(), expected);
        let encoded = serde_json::to_string(&method).expect("method should encode");
        assert_eq!(encoded, format!("\"{expected}\""));
    }

    let request = AgentKernelRequest::new(
        "request-1",
        AgentKernelMethod::Initialize,
        AgentKernelInitializeParams {
            protocol_version: PROTOCOL_VERSION,
            capabilities: vec![
                AgentKernelCapability::Events,
                AgentKernelCapability::StructuredErrors,
            ],
            host_capabilities: Vec::new(),
        },
    )
    .expect("request should encode")
    .with_deadline_ms(30_000);
    request.validate().expect("request should validate");
    let decoded: AgentKernelRequest =
        serde_json::from_str(&serde_json::to_string(&request).expect("request should serialize"))
            .expect("request should deserialize");
    assert_eq!(decoded.protocol(), PROTOCOL_NAME);
    assert_eq!(decoded.version(), PROTOCOL_VERSION);
    assert_eq!(decoded.request_id(), "request-1");
    assert_eq!(decoded.method(), AgentKernelMethod::Initialize);
    assert_eq!(decoded.deadline_ms(), Some(30_000));

    let response = AgentKernelResponse::success(
        "request-1",
        AgentKernelInitializeResult {
            protocol: PROTOCOL_NAME.to_string(),
            version: PROTOCOL_VERSION,
            capabilities: vec![AgentKernelCapability::Events],
            accepted_host_capabilities: Vec::new(),
        },
    )
    .expect("response should encode");
    response.validate().expect("response should validate");
    let message = AgentKernelMessage::Response { response };
    let decoded: AgentKernelMessage =
        serde_json::from_str(&serde_json::to_string(&message).expect("message should encode"))
            .expect("message should decode");
    decoded.validate().expect("message should validate");
}

#[test]
fn command_receipt_and_every_event_variant_round_trip() {
    let commands = [
        AgentKernelCommand::SubmitTurn {
            agent_id: 1,
            turn_id: 7,
            request: Box::new(turn_request()),
        },
        AgentKernelCommand::Interrupt {
            agent_id: 1,
            target: Some(target()),
        },
        AgentKernelCommand::RespondPermission {
            agent_id: 1,
            target: Some(target()),
            request_id: "permission-1".to_string(),
            option_id: Some("allow".to_string()),
        },
    ];
    for (index, command) in commands.into_iter().enumerate() {
        let command = AgentKernelCommandParams {
            command_id: u64::try_from(index + 1).expect("small command index"),
            command,
        };
        command.validate().expect("command should validate");
        let decoded: AgentKernelCommandParams =
            serde_json::from_str(&serde_json::to_string(&command).expect("command should encode"))
                .expect("command should decode");
        decoded.validate().expect("decoded command should validate");
    }

    for receipt in [
        AgentKernelCommandReceipt::Accepted,
        AgentKernelCommandReceipt::TurnStarted {
            turn_id: 7,
            target: target(),
            activity_label: "model".to_string(),
        },
        AgentKernelCommandReceipt::Interrupted {
            target: Some(target()),
        },
    ] {
        let result = AgentKernelCommandResult {
            command_id: 1,
            receipt,
        };
        result.validate().expect("receipt should validate");
        let decoded: AgentKernelCommandResult =
            serde_json::from_str(&serde_json::to_string(&result).expect("receipt should encode"))
                .expect("receipt should decode");
        decoded.validate().expect("decoded receipt should validate");
    }

    let tool_update = AgentKernelToolActivityUpdate {
        activity_id: "activity-1".to_string(),
        title: Some("private title".to_string()),
        kind: Some(AgentKernelToolKind::Read),
        status: Some(AgentKernelToolStatus::InProgress),
        content: Some(vec![AgentKernelToolContent::Text(
            "private tool content".to_string(),
        )]),
        locations: Some(vec![AgentKernelToolLocation {
            path: "/private/path".to_string(),
            line: Some(3),
        }]),
        raw_input: Some(serde_json::json!({"secret": "input"})),
        raw_output: Some(serde_json::json!({"secret": "output"})),
    };
    let variants = vec![
        AgentKernelEventKind::SystemMessage {
            message: "system".to_string(),
        },
        AgentKernelEventKind::Retrying {
            message: "retry".to_string(),
        },
        AgentKernelEventKind::OutputTokenEstimate { total_tokens: 3 },
        AgentKernelEventKind::InputTokenEstimate { total_tokens: 4 },
        AgentKernelEventKind::Thinking { is_thinking: true },
        AgentKernelEventKind::AssistantDelta {
            content: "assistant".to_string(),
        },
        AgentKernelEventKind::ReasoningDelta {
            content: "reasoning".to_string(),
        },
        AgentKernelEventKind::ToolActivityStarted {
            activity: AgentKernelToolActivity {
                activity_id: "activity-1".to_string(),
                title: "private title".to_string(),
                kind: AgentKernelToolKind::Read,
                status: AgentKernelToolStatus::Pending,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: None,
                raw_output: None,
            },
        },
        AgentKernelEventKind::ToolActivityUpdated {
            update: tool_update.clone(),
        },
        AgentKernelEventKind::TerminalUpdated {
            snapshot: AgentKernelTerminalSnapshot {
                terminal_id: "terminal-1".to_string(),
                command: Some("private command".to_string()),
                cwd: Some("/private/cwd".to_string()),
                output: "private output".to_string(),
                truncated: false,
                exit_status: None,
                released: false,
            },
        },
        AgentKernelEventKind::PermissionRequested {
            request: AgentKernelPermissionRequest {
                request_id: "permission-1".to_string(),
                title: Some("private permission title".to_string()),
                tool_activity: Some(tool_update),
                options: vec![AgentKernelPermissionOption {
                    option_id: "allow".to_string(),
                    name: "private option".to_string(),
                    kind: AgentKernelPermissionOptionKind::AllowOnce,
                }],
            },
        },
        AgentKernelEventKind::PreparationWarning {
            message: "warning".to_string(),
        },
        AgentKernelEventKind::TurnFinished {
            items: vec![ConversationItem::text(Role::Assistant, "finished")],
            reasoning_duration_ms: Some(2),
            metrics: Some(AgentKernelRequestMetrics {
                latency_ms: 1,
                output_tokens: 2,
                duration_ms: 3,
            }),
            context_usage: Some(AgentKernelContextUsage {
                limit: 128_000,
                used: 100,
            }),
        },
        AgentKernelEventKind::TurnFailed {
            message: "failed".to_string(),
        },
        AgentKernelEventKind::TurnInterrupted,
    ];

    for (index, kind) in variants.into_iter().enumerate() {
        let notification = AgentKernelEventNotification::new(
            u64::try_from(index + 1).expect("small index"),
            1,
            turn_event(kind),
        );
        notification
            .validate()
            .expect("notification should validate");
        let message = AgentKernelMessage::Event {
            event: notification,
        };
        let decoded: AgentKernelMessage =
            serde_json::from_str(&serde_json::to_string(&message).expect("event should encode"))
                .expect("event should decode");
        decoded.validate().expect("event should validate");
    }
}

#[test]
fn validation_rejects_invalid_identity_capability_and_payload_shapes() {
    let duplicate = AgentKernelInitializeParams {
        protocol_version: PROTOCOL_VERSION,
        capabilities: vec![AgentKernelCapability::Events, AgentKernelCapability::Events],
        host_capabilities: Vec::new(),
    };
    assert_eq!(
        duplicate.validate(),
        Err(AgentKernelValidationError::DuplicateCapability)
    );

    let duplicate_host = AgentKernelInitializeResult {
        protocol: PROTOCOL_NAME.to_string(),
        version: PROTOCOL_VERSION,
        capabilities: Vec::new(),
        accepted_host_capabilities: vec![
            AgentKernelHostCapability::Llm,
            AgentKernelHostCapability::Llm,
        ],
    };
    assert_eq!(
        duplicate_host.validate(),
        Err(AgentKernelValidationError::DuplicateHostCapability)
    );

    let zero_command = AgentKernelCommandParams {
        command_id: 0,
        command: AgentKernelCommand::Interrupt {
            agent_id: 1,
            target: None,
        },
    };
    assert_eq!(
        zero_command.validate(),
        Err(AgentKernelValidationError::InvalidCommandId)
    );

    let invalid_permission = AgentKernelEventKind::PermissionRequested {
        request: AgentKernelPermissionRequest {
            request_id: "permission".to_string(),
            title: None,
            tool_activity: None,
            options: Vec::new(),
        },
    };
    assert_eq!(
        turn_event(invalid_permission).validate(),
        Err(AgentKernelValidationError::InvalidPermission)
    );

    assert_eq!(
        AgentKernelEventNotification::new(0, 1, turn_event(AgentKernelEventKind::TurnInterrupted),)
            .validate(),
        Err(AgentKernelValidationError::InvalidEventSequence)
    );
}

#[test]
fn diagnostics_redact_instruction_delivery_and_remote_controlled_bodies() {
    let command = AgentKernelCommandParams {
        command_id: 41,
        command: AgentKernelCommand::SubmitTurn {
            agent_id: 42,
            turn_id: 43,
            request: Box::new(turn_request()),
        },
    };
    let permission = AgentKernelPermissionRequest {
        request_id: "secret-permission-id".to_string(),
        title: Some("secret permission title".to_string()),
        tool_activity: None,
        options: vec![AgentKernelPermissionOption {
            option_id: "secret-option-id".to_string(),
            name: "secret option name".to_string(),
            kind: AgentKernelPermissionOptionKind::AllowOnce,
        }],
    };
    let terminal = AgentKernelTerminalSnapshot {
        terminal_id: "secret-terminal-id".to_string(),
        command: Some("secret command".to_string()),
        cwd: Some("/secret/cwd".to_string()),
        output: "secret terminal output".to_string(),
        truncated: false,
        exit_status: Some(AgentKernelTerminalExitStatus {
            exit_code: None,
            signal: Some("secret signal".to_string()),
        }),
        released: false,
    };
    let error = AgentKernelError::new(AgentKernelErrorCode::Internal, "secret remote error", false)
        .with_details(serde_json::json!({"secret": "/secret/path"}))
        .expect("details should encode");
    let diagnostics = [
        format!("{command:?}"),
        format!("{permission:?}"),
        format!("{terminal:?}"),
        format!("{error:?}"),
    ]
    .join("\n");

    for forbidden in [
        "visible delivery sentinel",
        "provider-visible content sentinel",
        "private-skill",
        "private-prompt",
        "/private/SKILL.md",
        "private image bytes",
        "secret-permission-id",
        "secret permission title",
        "secret-option-id",
        "secret option name",
        "secret-terminal-id",
        "secret command",
        "/secret/cwd",
        "secret terminal output",
        "secret signal",
        "secret remote error",
        "/secret/path",
        "41",
        "42",
        "43",
    ] {
        assert!(!diagnostics.contains(forbidden), "leaked {forbidden}");
    }
}

#[test]
fn failure_response_drops_remote_body_from_debug_but_preserves_wire_value() {
    let response = AgentKernelResponse::failure(
        "request-secret",
        AgentKernelError::new(
            AgentKernelErrorCode::CommandRejected,
            "remote body sentinel",
            false,
        ),
    );
    response.validate().expect("response should validate");
    let debug = format!("{response:?}");
    assert!(!debug.contains("request-secret"));
    assert!(!debug.contains("remote body sentinel"));

    let json = serde_json::to_string(&response).expect("response should encode");
    assert!(json.contains("remote body sentinel"));
}
