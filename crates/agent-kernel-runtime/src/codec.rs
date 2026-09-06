use std::time::Duration;

use agent_kernel_protocol::{
    AgentKernelCommand, AgentKernelErrorCode, AgentKernelEvent, AgentKernelEventKind,
    AgentKernelImageDetail, AgentKernelPermissionOption, AgentKernelPermissionOptionKind,
    AgentKernelPermissionRequest, AgentKernelPromptOrigin, AgentKernelRequestMetrics,
    AgentKernelSkillBinding, AgentKernelTarget, AgentKernelTerminalSnapshot,
    AgentKernelToolActivity, AgentKernelToolActivityUpdate, AgentKernelToolContent,
    AgentKernelToolKind, AgentKernelToolLocation, AgentKernelToolStatus, AgentKernelTurnControls,
    AgentKernelTurnRequest, AgentKernelUserAttachment, AgentKernelUserDelivery,
};
use provider_protocol::ImageDetail;
use runtime_domain::{
    agent::{AgentCommand, AgentEvent, AgentEventKind, AgentRuntimeError},
    context_budget::{ContextTokenLimit, ContextWindowUsage},
    prompt_assembly::PromptSourceOrigin,
    session::{
        ConversationResponse, RuntimePermissionOption, RuntimePermissionOptionKind,
        RuntimePermissionRequest, RuntimeRequestMetrics, RuntimeTarget, RuntimeTerminalExitStatus,
        RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityLocation, RuntimeToolActivityRawValue, RuntimeToolActivityStatus,
        RuntimeToolActivityUpdate, RuntimeToolKind, TranscriptCustomPromptBinding,
        TranscriptSkillBinding, TranscriptUserAttachment,
    },
};

use super::{CommandExpectation, TurnIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CodecError;

pub(super) fn encode_command(
    command: AgentCommand,
) -> Result<(AgentKernelCommand, CommandExpectation), AgentRuntimeError> {
    match command {
        AgentCommand::SubmitTurn {
            agent_id,
            turn_id,
            request,
        } => {
            let (conversation_request, transcript, _) = request.into_parts();
            if agent_id.get() == 0 || turn_id.get() == 0 || !conversation_request.is_user_message()
            {
                return Err(AgentRuntimeError::CommandRejected(
                    "External Agent command identity is invalid".to_string(),
                ));
            }
            let target = conversation_request.target();
            let provider_content = conversation_request
                .message()
                .content_blocks()
                .ok_or_else(|| {
                    AgentRuntimeError::CommandRejected(
                        "External Agent turn content is invalid".to_string(),
                    )
                })?
                .to_vec();
            let wire_request = AgentKernelTurnRequest {
                target: encode_target(&target),
                delivery: AgentKernelUserDelivery {
                    content: transcript.content,
                    attachments: transcript
                        .attachments
                        .into_iter()
                        .map(encode_attachment)
                        .collect(),
                },
                controls: AgentKernelTurnControls {
                    skill_bindings: transcript
                        .skill_bindings
                        .into_iter()
                        .map(encode_skill_binding)
                        .collect::<Result<_, _>>()?,
                    custom_prompt_bindings: transcript
                        .custom_prompt_bindings
                        .into_iter()
                        .map(encode_custom_prompt_binding)
                        .collect::<Result<_, _>>()?,
                },
                provider_content,
            };
            wire_request.validate().map_err(|_| {
                AgentRuntimeError::CommandRejected(
                    "External Agent turn content is invalid".to_string(),
                )
            })?;
            let identity = TurnIdentity {
                agent_id,
                turn_id,
                target,
            };
            Ok((
                AgentKernelCommand::SubmitTurn {
                    agent_id: agent_id.get(),
                    turn_id: turn_id.get(),
                    request: Box::new(wire_request),
                },
                CommandExpectation::Submit(identity),
            ))
        }
        AgentCommand::Interrupt { agent_id, target } => Ok((
            AgentKernelCommand::Interrupt {
                agent_id: agent_id.get(),
                target: target.as_ref().map(encode_target),
            },
            CommandExpectation::Interrupt {
                agent_id,
                requested_target: target,
                turn: None,
            },
        )),
        AgentCommand::RespondPermission {
            agent_id,
            target,
            request_id,
            option_id,
        } => Ok((
            AgentKernelCommand::RespondPermission {
                agent_id: agent_id.get(),
                target: target.as_ref().map(encode_target),
                request_id: request_id.clone(),
                option_id,
            },
            CommandExpectation::Permission {
                agent_id,
                requested_target: target,
                turn: None,
                request_id,
            },
        )),
    }
}

pub(super) fn decode_event(event: AgentKernelEvent) -> Result<AgentEvent, CodecError> {
    let target = decode_target(event.target)?;
    let kind = match event.kind {
        AgentKernelEventKind::SystemMessage { message } => {
            AgentEventKind::SystemMessage { message }
        }
        AgentKernelEventKind::Retrying { message } => AgentEventKind::Retrying { message },
        AgentKernelEventKind::OutputTokenEstimate { total_tokens } => {
            AgentEventKind::OutputTokenEstimate {
                total_tokens: usize::try_from(total_tokens).map_err(|_| CodecError)?,
            }
        }
        AgentKernelEventKind::InputTokenEstimate { total_tokens } => {
            AgentEventKind::InputTokenEstimate {
                total_tokens: usize::try_from(total_tokens).map_err(|_| CodecError)?,
            }
        }
        AgentKernelEventKind::Thinking { is_thinking } => AgentEventKind::Thinking { is_thinking },
        AgentKernelEventKind::AssistantDelta { content } => {
            AgentEventKind::AssistantDelta { content }
        }
        AgentKernelEventKind::ReasoningDelta { content } => {
            AgentEventKind::ReasoningDelta { content }
        }
        AgentKernelEventKind::ToolActivityStarted { activity } => {
            AgentEventKind::ToolActivityStarted {
                activity: decode_tool_activity(activity),
            }
        }
        AgentKernelEventKind::ToolActivityUpdated { update } => {
            AgentEventKind::ToolActivityUpdated {
                update: decode_tool_update(update),
            }
        }
        AgentKernelEventKind::TerminalUpdated { snapshot } => AgentEventKind::TerminalUpdated {
            snapshot: decode_terminal_snapshot(snapshot),
        },
        AgentKernelEventKind::PermissionRequested { request } => {
            AgentEventKind::PermissionRequested {
                request: decode_permission_request(request),
            }
        }
        AgentKernelEventKind::PreparationWarning { message } => {
            AgentEventKind::PreparationWarning { message }
        }
        AgentKernelEventKind::TurnFinished {
            items,
            reasoning_duration_ms,
            metrics,
            context_usage,
        } => AgentEventKind::TurnFinished {
            response: ConversationResponse::new(
                items,
                reasoning_duration_ms.map(Duration::from_millis),
            ),
            metrics: metrics.map(decode_metrics).transpose()?,
            context_usage: context_usage.map(decode_context_usage).transpose()?,
        },
        AgentKernelEventKind::TurnFailed { message } => AgentEventKind::TurnFailed { message },
        AgentKernelEventKind::TurnInterrupted => AgentEventKind::TurnInterrupted,
    };
    Ok(AgentEvent {
        agent_id: runtime_domain::agent::AgentId::new(event.agent_id),
        turn_id: runtime_domain::agent::AgentTurnId::new(event.turn_id),
        target,
        kind,
    })
}

pub(super) fn encode_target(target: &RuntimeTarget) -> AgentKernelTarget {
    let RuntimeTarget::Provider(target) = target;
    AgentKernelTarget {
        provider_id: target.provider_id.clone(),
        model_id: target.model_id.clone(),
    }
}

pub(super) fn decode_target(target: AgentKernelTarget) -> Result<RuntimeTarget, CodecError> {
    target.validate().map_err(|_| CodecError)?;
    Ok(RuntimeTarget::provider(target.provider_id, target.model_id))
}

pub(super) fn decode_optional_target(
    target: Option<AgentKernelTarget>,
) -> Result<Option<RuntimeTarget>, CodecError> {
    target.map(decode_target).transpose()
}

pub(super) fn map_remote_error(code: AgentKernelErrorCode) -> AgentRuntimeError {
    match code {
        AgentKernelErrorCode::Busy => AgentRuntimeError::Busy,
        AgentKernelErrorCode::UnknownAgent => AgentRuntimeError::UnknownAgent,
        AgentKernelErrorCode::InvalidRequest
        | AgentKernelErrorCode::UnsupportedVersion
        | AgentKernelErrorCode::CapabilityDenied
        | AgentKernelErrorCode::CommandRejected
        | AgentKernelErrorCode::Internal => AgentRuntimeError::CommandRejected(
            "External Agent kernel rejected the command".to_string(),
        ),
    }
}

fn encode_attachment(attachment: TranscriptUserAttachment) -> AgentKernelUserAttachment {
    match attachment {
        TranscriptUserAttachment::Image {
            data_base64,
            mime_type,
            uri,
            detail,
        } => AgentKernelUserAttachment::Image {
            data_base64,
            mime_type,
            uri,
            detail: detail.map(encode_image_detail),
        },
    }
}

fn encode_image_detail(detail: ImageDetail) -> AgentKernelImageDetail {
    match detail {
        ImageDetail::Auto => AgentKernelImageDetail::Auto,
        ImageDetail::Low => AgentKernelImageDetail::Low,
        ImageDetail::High => AgentKernelImageDetail::High,
        ImageDetail::Original => AgentKernelImageDetail::Original,
    }
}

fn encode_skill_binding(
    binding: TranscriptSkillBinding,
) -> Result<AgentKernelSkillBinding, AgentRuntimeError> {
    Ok(AgentKernelSkillBinding {
        skill_name: binding.skill_name,
        origin: encode_origin(binding.origin),
        skill_path: binding.skill_path,
        start_char: u64::try_from(binding.start_char).map_err(|_| invalid_control())?,
        end_char: u64::try_from(binding.end_char).map_err(|_| invalid_control())?,
    })
}

fn encode_custom_prompt_binding(
    binding: TranscriptCustomPromptBinding,
) -> Result<agent_kernel_protocol::AgentKernelCustomPromptBinding, AgentRuntimeError> {
    Ok(agent_kernel_protocol::AgentKernelCustomPromptBinding {
        reference_id: binding.reference_id,
        origin: encode_origin(binding.origin),
        start_char: u64::try_from(binding.start_char).map_err(|_| invalid_control())?,
        end_char: u64::try_from(binding.end_char).map_err(|_| invalid_control())?,
    })
}

fn encode_origin(origin: PromptSourceOrigin) -> AgentKernelPromptOrigin {
    match origin {
        PromptSourceOrigin::Builtin => AgentKernelPromptOrigin::Builtin,
        PromptSourceOrigin::Global => AgentKernelPromptOrigin::Global,
        PromptSourceOrigin::Project => AgentKernelPromptOrigin::Project,
    }
}

fn invalid_control() -> AgentRuntimeError {
    AgentRuntimeError::CommandRejected("External Agent controls are invalid".to_string())
}

fn decode_tool_activity(activity: AgentKernelToolActivity) -> RuntimeToolActivity {
    RuntimeToolActivity {
        activity_id: activity.activity_id,
        title: activity.title,
        kind: decode_tool_kind(activity.kind),
        status: decode_tool_status(activity.status),
        content: activity
            .content
            .into_iter()
            .map(decode_tool_content)
            .collect(),
        locations: activity
            .locations
            .into_iter()
            .map(decode_tool_location)
            .collect(),
        raw_input: activity.raw_input.map(RuntimeToolActivityRawValue::new),
        raw_output: activity.raw_output.map(RuntimeToolActivityRawValue::new),
    }
}

fn decode_tool_update(update: AgentKernelToolActivityUpdate) -> RuntimeToolActivityUpdate {
    RuntimeToolActivityUpdate {
        activity_id: update.activity_id,
        title: update.title,
        kind: update.kind.map(decode_tool_kind),
        status: update.status.map(decode_tool_status),
        content: update
            .content
            .map(|content| content.into_iter().map(decode_tool_content).collect()),
        locations: update
            .locations
            .map(|locations| locations.into_iter().map(decode_tool_location).collect()),
        raw_input: update.raw_input.map(RuntimeToolActivityRawValue::new),
        raw_output: update.raw_output.map(RuntimeToolActivityRawValue::new),
    }
}

fn decode_tool_kind(kind: AgentKernelToolKind) -> RuntimeToolKind {
    match kind {
        AgentKernelToolKind::Read => RuntimeToolKind::Read,
        AgentKernelToolKind::Write => RuntimeToolKind::Write,
        AgentKernelToolKind::Edit => RuntimeToolKind::Edit,
        AgentKernelToolKind::Delete => RuntimeToolKind::Delete,
        AgentKernelToolKind::Move => RuntimeToolKind::Move,
        AgentKernelToolKind::Search => RuntimeToolKind::Search,
        AgentKernelToolKind::Execute => RuntimeToolKind::Execute,
        AgentKernelToolKind::Think => RuntimeToolKind::Think,
        AgentKernelToolKind::Fetch => RuntimeToolKind::Fetch,
        AgentKernelToolKind::SwitchMode => RuntimeToolKind::SwitchMode,
        AgentKernelToolKind::Other => RuntimeToolKind::Other,
    }
}

fn decode_tool_status(status: AgentKernelToolStatus) -> RuntimeToolActivityStatus {
    match status {
        AgentKernelToolStatus::Pending => RuntimeToolActivityStatus::Pending,
        AgentKernelToolStatus::InProgress => RuntimeToolActivityStatus::InProgress,
        AgentKernelToolStatus::Completed => RuntimeToolActivityStatus::Completed,
        AgentKernelToolStatus::Failed => RuntimeToolActivityStatus::Failed,
    }
}

fn decode_tool_content(content: AgentKernelToolContent) -> RuntimeToolActivityContent {
    match content {
        AgentKernelToolContent::Text(text) => RuntimeToolActivityContent::Text(text),
        AgentKernelToolContent::Image { mime_type, uri } => {
            RuntimeToolActivityContent::Image { mime_type, uri }
        }
        AgentKernelToolContent::Audio { mime_type } => {
            RuntimeToolActivityContent::Audio { mime_type }
        }
        AgentKernelToolContent::ResourceLink { uri, name, title } => {
            RuntimeToolActivityContent::ResourceLink { uri, name, title }
        }
        AgentKernelToolContent::Resource {
            uri,
            mime_type,
            text,
        } => RuntimeToolActivityContent::Resource {
            uri,
            mime_type,
            text,
        },
        AgentKernelToolContent::Diff {
            path,
            old_text,
            new_text,
            is_truncated,
        } => RuntimeToolActivityContent::Diff {
            path,
            old_text,
            new_text,
            is_truncated,
        },
        AgentKernelToolContent::Terminal { terminal_id } => {
            RuntimeToolActivityContent::Terminal { terminal_id }
        }
        AgentKernelToolContent::Unknown(value) => RuntimeToolActivityContent::Unknown(value),
    }
}

fn decode_tool_location(location: AgentKernelToolLocation) -> RuntimeToolActivityLocation {
    RuntimeToolActivityLocation {
        path: location.path,
        line: location.line,
    }
}

fn decode_terminal_snapshot(snapshot: AgentKernelTerminalSnapshot) -> RuntimeTerminalSnapshot {
    RuntimeTerminalSnapshot {
        terminal_id: snapshot.terminal_id,
        command: snapshot.command,
        cwd: snapshot.cwd,
        output: snapshot.output,
        truncated: snapshot.truncated,
        exit_status: snapshot
            .exit_status
            .map(|status| RuntimeTerminalExitStatus {
                exit_code: status.exit_code,
                signal: status.signal,
            }),
        released: snapshot.released,
    }
}

fn decode_permission_request(request: AgentKernelPermissionRequest) -> RuntimePermissionRequest {
    RuntimePermissionRequest {
        request_id: request.request_id,
        title: request.title,
        tool_activity: request.tool_activity.map(decode_tool_update),
        options: request
            .options
            .into_iter()
            .map(decode_permission_option)
            .collect(),
    }
}

fn decode_permission_option(option: AgentKernelPermissionOption) -> RuntimePermissionOption {
    RuntimePermissionOption {
        option_id: option.option_id,
        name: option.name,
        kind: match option.kind {
            AgentKernelPermissionOptionKind::AllowOnce => RuntimePermissionOptionKind::AllowOnce,
            AgentKernelPermissionOptionKind::AllowAlways => {
                RuntimePermissionOptionKind::AllowAlways
            }
            AgentKernelPermissionOptionKind::RejectOnce => RuntimePermissionOptionKind::RejectOnce,
            AgentKernelPermissionOptionKind::RejectAlways => {
                RuntimePermissionOptionKind::RejectAlways
            }
            AgentKernelPermissionOptionKind::Unknown => RuntimePermissionOptionKind::Unknown,
        },
    }
}

fn decode_metrics(metrics: AgentKernelRequestMetrics) -> Result<RuntimeRequestMetrics, CodecError> {
    Ok(RuntimeRequestMetrics::new(
        Duration::from_millis(metrics.latency_ms),
        usize::try_from(metrics.output_tokens).map_err(|_| CodecError)?,
        Duration::from_millis(metrics.duration_ms),
    ))
}

fn decode_context_usage(
    usage: agent_kernel_protocol::AgentKernelContextUsage,
) -> Result<ContextWindowUsage, CodecError> {
    let limit = usize::try_from(usage.limit).map_err(|_| CodecError)?;
    let used = usize::try_from(usage.used).map_err(|_| CodecError)?;
    Ok(ContextWindowUsage {
        limit: ContextTokenLimit::try_from(limit).map_err(|_| CodecError)?,
        used,
    })
}
