//! Conversation event 到 TUI runtime event 的转换。

use runtime_domain::session::{
    ConversationEvent, RuntimeEvent, RuntimeRequestMetrics, RuntimeTarget,
};

pub(crate) fn runtime_event_from_conversation_event(
    target: Option<RuntimeTarget>,
    event: ConversationEvent,
) -> RuntimeEvent {
    match event {
        ConversationEvent::SystemMessage { message } => {
            RuntimeEvent::SystemMessage { target, message }
        }
        ConversationEvent::Retrying { message } => RuntimeEvent::Retrying { target, message },
        ConversationEvent::OutputTokenEstimate { total_tokens } => {
            RuntimeEvent::OutputTokenEstimate {
                target,
                total_tokens,
            }
        }
        ConversationEvent::InputTokenEstimate { total_tokens } => {
            RuntimeEvent::InputTokenEstimate {
                target,
                total_tokens,
            }
        }
        ConversationEvent::Thinking { is_thinking } => RuntimeEvent::Thinking {
            target,
            is_thinking,
        },
        ConversationEvent::AssistantDelta { content } => match target {
            Some(target) => RuntimeEvent::AssistantDelta { target, content },
            None => missing_target_event("assistant delta"),
        },
        ConversationEvent::ReasoningDelta { content } => match target {
            Some(target) => RuntimeEvent::ReasoningDelta { target, content },
            None => missing_target_event("reasoning delta"),
        },
        ConversationEvent::ToolActivityStarted { activity } => match target {
            Some(target) => RuntimeEvent::ToolActivityStarted { target, activity },
            None => missing_target_event("tool activity start"),
        },
        ConversationEvent::ToolActivityUpdated { update } => match target {
            Some(target) => RuntimeEvent::ToolActivityUpdated { target, update },
            None => missing_target_event("tool activity update"),
        },
        ConversationEvent::TerminalUpdated { snapshot } => match target {
            Some(target) => RuntimeEvent::TerminalUpdated { target, snapshot },
            None => missing_target_event("terminal update"),
        },
        ConversationEvent::ManagedSearchToolAuthorization { .. } => RuntimeEvent::SystemMessage {
            target,
            message: "managed search tool authorization was not persisted".to_string(),
        },
        ConversationEvent::PermissionRequested { request } => match target {
            Some(target) => RuntimeEvent::PermissionRequested { target, request },
            None => missing_target_event("permission request"),
        },
        ConversationEvent::Finished { response, metrics } => RuntimeEvent::MessageFinished {
            target,
            response,
            finish_reason: None,
            metrics: metrics.map(|metrics| {
                RuntimeRequestMetrics::new(metrics.latency, metrics.output_tokens, metrics.duration)
            }),
        },
        ConversationEvent::Failed { message } => RuntimeEvent::Failed { target, message },
        ConversationEvent::Interrupted => RuntimeEvent::Interrupted { target },
    }
}

fn missing_target_event(event_name: &str) -> RuntimeEvent {
    RuntimeEvent::Failed {
        target: None,
        message: format!("Conversation target is missing for {event_name}"),
    }
}

pub(crate) fn should_defer_runtime_event_for_render_barrier(
    current_batch: &[RuntimeEvent],
    next_event: &RuntimeEvent,
) -> bool {
    matches!(next_event, RuntimeEvent::PermissionRequested { .. })
        && current_batch.iter().any(is_runtime_token_estimate)
}

fn is_runtime_token_estimate(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::OutputTokenEstimate { .. } | RuntimeEvent::InputTokenEstimate { .. }
    )
}
