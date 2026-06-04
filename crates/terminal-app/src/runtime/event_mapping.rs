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
        ConversationEvent::AssistantDelta { content } => RuntimeEvent::AssistantDelta {
            target: target.expect("conversation target should be available for assistant delta"),
            content,
        },
        ConversationEvent::ReasoningDelta { content } => RuntimeEvent::ReasoningDelta {
            target: target.expect("conversation target should be available for reasoning delta"),
            content,
        },
        ConversationEvent::ToolActivityStarted { activity } => RuntimeEvent::ToolActivityStarted {
            target: target.expect("conversation target should be available for tool activity"),
            activity,
        },
        ConversationEvent::ToolActivityUpdated { update } => RuntimeEvent::ToolActivityUpdated {
            target: target.expect("conversation target should be available for tool activity"),
            update,
        },
        ConversationEvent::TerminalUpdated { snapshot } => RuntimeEvent::TerminalUpdated {
            target: target.expect("conversation target should be available for terminal update"),
            snapshot,
        },
        ConversationEvent::ManagedSearchToolAuthorization { .. } => RuntimeEvent::SystemMessage {
            target,
            message: "managed search tool authorization was not persisted".to_string(),
        },
        ConversationEvent::PermissionRequested { request } => RuntimeEvent::PermissionRequested {
            target: target.expect("conversation target should be available for permission request"),
            request,
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
