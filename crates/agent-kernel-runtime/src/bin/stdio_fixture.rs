//! stdio transport contract tests 使用的 self-contained Agent kernel fixture。

use std::{
    io::{self, Write},
    time::Duration,
};

use agent_kernel_protocol::{
    AgentKernelCapability, AgentKernelCommand, AgentKernelCommandParams, AgentKernelCommandReceipt,
    AgentKernelCommandResult, AgentKernelEvent, AgentKernelEventKind, AgentKernelEventNotification,
    AgentKernelInitializeResult, AgentKernelMessage, AgentKernelMethod,
    AgentKernelPermissionOption, AgentKernelPermissionOptionKind, AgentKernelPermissionRequest,
    AgentKernelRequest, AgentKernelRequestMetrics, AgentKernelResponse, AgentKernelShutdownResult,
    AgentKernelTarget,
};
use provider_protocol::{ConversationItem, Role};
use stdio_framing::FrameCodec;

struct ActiveTurn {
    agent_id: u64,
    turn_id: u64,
    target: AgentKernelTarget,
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().unwrap_or_default();
    let auxiliary_path = arguments.next();
    if mode == "exit" {
        return;
    }
    if mode == "hold" {
        std::thread::sleep(Duration::from_secs(3));
        return;
    }
    if mode == "stderr" {
        eprintln!("fixture stderr must never cross the transport boundary");
        return;
    }
    if mode == "environment"
        && (std::env::var_os("HUNEA_AGENT_KERNEL_ALLOWED").as_deref()
            != Some(std::ffi::OsStr::new("explicit"))
            || std::env::var_os("PATH").is_some())
    {
        return;
    }
    if mode == "ignore-shutdown" {
        let Some(path) = auxiliary_path else {
            return;
        };
        if std::fs::write(path, std::process::id().to_string()).is_err() {
            return;
        }
    }

    let codec = FrameCodec::default();
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin = stdin.lock();
    let mut stdout = stdout.lock();
    let mut sequence = 0_u64;
    let mut active_turn: Option<ActiveTurn> = None;
    let mut delayed_command = false;

    while let Ok(request) = codec.read_json::<_, AgentKernelRequest>(&mut stdin) {
        let request_id = request.request_id().to_string();
        match request.method() {
            AgentKernelMethod::Initialize => {
                if mode == "oversized" {
                    let _ = stdout.write_all(b"Content-Length: 1000000\r\n\r\n");
                    let _ = stdout.flush();
                    return;
                }
                let response = AgentKernelResponse::success(
                    request_id,
                    AgentKernelInitializeResult {
                        protocol: agent_kernel_protocol::PROTOCOL_NAME.to_string(),
                        version: agent_kernel_protocol::PROTOCOL_VERSION,
                        capabilities: vec![
                            AgentKernelCapability::Events,
                            AgentKernelCapability::Interrupt,
                            AgentKernelCapability::PermissionResponse,
                            AgentKernelCapability::StructuredErrors,
                        ],
                        accepted_host_capabilities: Vec::new(),
                    },
                );
                if write_response(&codec, &mut stdout, response).is_err() {
                    return;
                }
            }
            AgentKernelMethod::AgentCommand => {
                let Ok(params) = request.decode_params::<AgentKernelCommandParams>() else {
                    return;
                };
                match params.command {
                    AgentKernelCommand::SubmitTurn {
                        agent_id,
                        turn_id,
                        request,
                    } => {
                        let turn = ActiveTurn {
                            agent_id,
                            turn_id,
                            target: request.target.clone(),
                        };
                        let should_delay_response = mode == "late-response" && !delayed_command;
                        if should_delay_response {
                            delayed_command = true;
                            std::thread::sleep(Duration::from_millis(150));
                        }
                        let delta = (!should_delay_response).then(|| {
                            event(
                                &mut sequence,
                                params.command_id,
                                &turn,
                                AgentKernelEventKind::AssistantDelta {
                                    content: "remote stream".to_string(),
                                },
                            )
                        });
                        if mode == "event-before-receipt"
                            && write_event(
                                &codec,
                                &mut stdout,
                                delta.clone().expect("non-delayed command has a delta"),
                            )
                            .is_err()
                        {
                            return;
                        }
                        let response = AgentKernelResponse::success(
                            request_id,
                            AgentKernelCommandResult {
                                command_id: params.command_id,
                                receipt: AgentKernelCommandReceipt::TurnStarted {
                                    turn_id,
                                    target: turn.target.clone(),
                                    activity_label: turn.target.model_id.clone(),
                                },
                            },
                        );
                        if write_response(&codec, &mut stdout, response).is_err() {
                            return;
                        }
                        if should_delay_response {
                            continue;
                        }
                        if mode != "event-before-receipt"
                            && write_event(
                                &codec,
                                &mut stdout,
                                delta.expect("non-delayed command has a delta"),
                            )
                            .is_err()
                        {
                            return;
                        }
                        if mode == "event-flood" {
                            std::thread::sleep(Duration::from_millis(20));
                            for index in 0..512 {
                                let flooded = event(
                                    &mut sequence,
                                    params.command_id,
                                    &turn,
                                    AgentKernelEventKind::AssistantDelta {
                                        content: format!("flood-{index}"),
                                    },
                                );
                                if write_event(&codec, &mut stdout, flooded).is_err() {
                                    return;
                                }
                            }
                            std::thread::sleep(Duration::from_secs(2));
                            return;
                        }
                        let permission = event(
                            &mut sequence,
                            params.command_id,
                            &turn,
                            AgentKernelEventKind::PermissionRequested {
                                request: AgentKernelPermissionRequest {
                                    request_id: "permission-1".to_string(),
                                    title: Some("Allow remote action?".to_string()),
                                    tool_activity: None,
                                    options: vec![AgentKernelPermissionOption {
                                        option_id: "allow".to_string(),
                                        name: "Allow once".to_string(),
                                        kind: AgentKernelPermissionOptionKind::AllowOnce,
                                    }],
                                },
                            },
                        );
                        if write_event(&codec, &mut stdout, permission).is_err() {
                            return;
                        }
                        if mode == "invalid-sequence" {
                            std::thread::sleep(Duration::from_millis(20));
                            let invalid = AgentKernelEventNotification::new(
                                sequence.saturating_add(2),
                                params.command_id,
                                AgentKernelEvent {
                                    agent_id: turn.agent_id,
                                    turn_id: turn.turn_id,
                                    target: turn.target.clone(),
                                    kind: AgentKernelEventKind::AssistantDelta {
                                        content: "invalid event body".to_string(),
                                    },
                                },
                            );
                            let _ = write_event(&codec, &mut stdout, invalid);
                            std::thread::sleep(Duration::from_secs(2));
                            return;
                        }
                        active_turn = Some(turn);
                    }
                    AgentKernelCommand::RespondPermission { .. } => {
                        let Some(turn) = active_turn.as_ref() else {
                            return;
                        };
                        let response = AgentKernelResponse::success(
                            request_id,
                            AgentKernelCommandResult {
                                command_id: params.command_id,
                                receipt: AgentKernelCommandReceipt::Accepted,
                            },
                        );
                        if write_response(&codec, &mut stdout, response).is_err() {
                            return;
                        }
                        let finished = event(
                            &mut sequence,
                            params.command_id,
                            turn,
                            AgentKernelEventKind::TurnFinished {
                                items: vec![ConversationItem::text(Role::Assistant, "remote done")],
                                reasoning_duration_ms: None,
                                metrics: Some(AgentKernelRequestMetrics {
                                    latency_ms: 1,
                                    output_tokens: 2,
                                    duration_ms: 3,
                                }),
                                context_usage: None,
                            },
                        );
                        if write_event(&codec, &mut stdout, finished).is_err() {
                            return;
                        }
                    }
                    AgentKernelCommand::Interrupt { .. } => {
                        let response = AgentKernelResponse::success(
                            request_id,
                            AgentKernelCommandResult {
                                command_id: params.command_id,
                                receipt: AgentKernelCommandReceipt::Interrupted {
                                    target: active_turn.as_ref().map(|turn| turn.target.clone()),
                                },
                            },
                        );
                        if write_response(&codec, &mut stdout, response).is_err() {
                            return;
                        }
                        if let Some(turn) = active_turn.as_ref() {
                            let interrupted = event(
                                &mut sequence,
                                params.command_id,
                                turn,
                                AgentKernelEventKind::TurnInterrupted,
                            );
                            if write_event(&codec, &mut stdout, interrupted).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
            AgentKernelMethod::Shutdown => {
                if mode == "ignore-shutdown" {
                    std::thread::sleep(Duration::from_secs(3));
                    return;
                }
                let response = AgentKernelResponse::success(
                    request_id,
                    AgentKernelShutdownResult { drained: true },
                );
                let _ = write_response(&codec, &mut stdout, response);
                return;
            }
        }
    }
}

fn event(
    sequence: &mut u64,
    command_id: u64,
    turn: &ActiveTurn,
    kind: AgentKernelEventKind,
) -> AgentKernelEventNotification {
    *sequence = sequence.checked_add(1).expect("fixture sequence");
    AgentKernelEventNotification::new(
        *sequence,
        command_id,
        AgentKernelEvent {
            agent_id: turn.agent_id,
            turn_id: turn.turn_id,
            target: turn.target.clone(),
            kind,
        },
    )
}

fn write_response(
    codec: &FrameCodec,
    stdout: &mut impl io::Write,
    response: Result<AgentKernelResponse, agent_kernel_protocol::AgentKernelEncodeError>,
) -> Result<(), ()> {
    let response = response.map_err(|_| ())?;
    codec
        .write_json(stdout, &AgentKernelMessage::Response { response })
        .map_err(|_| ())
}

fn write_event(
    codec: &FrameCodec,
    stdout: &mut impl io::Write,
    event: AgentKernelEventNotification,
) -> Result<(), ()> {
    codec
        .write_json(stdout, &AgentKernelMessage::Event { event })
        .map_err(|_| ())
}
