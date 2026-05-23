use crate::Model;
#[cfg(test)]
use crate::runtime::RuntimeEventApply;
#[cfg(test)]
use runtime_domain::session::{
    ConversationEvent, RuntimeEvent, RuntimeRequestMetrics, RuntimeTarget,
};
use runtime_domain::session::{ConversationTurnRequest, RuntimeCommand, RuntimeCommandReceipt};

use super::RuntimeCoordinator;

#[cfg(test)]
pub(super) fn apply_conversation_event(
    model: &mut Model,
    target: Option<RuntimeTarget>,
    event: ConversationEvent,
) {
    let runtime_event = match event {
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
        ConversationEvent::PermissionRequested { request } => RuntimeEvent::PermissionRequested {
            target: target.expect("conversation target should be available for permission request"),
            request,
        },
        ConversationEvent::Finished { response, metrics } => RuntimeEvent::MessageFinished {
            target,
            content: response.content,
            reasoning_content: response.reasoning_content,
            reasoning_duration: response.reasoning_duration,
            finish_reason: None,
            metrics: metrics.map(|metrics| {
                RuntimeRequestMetrics::new(metrics.latency, metrics.output_tokens, metrics.duration)
            }),
        },
        ConversationEvent::Failed { message } => RuntimeEvent::Failed { target, message },
        ConversationEvent::Interrupted => RuntimeEvent::Interrupted { target },
    };
    model.apply_runtime_event(runtime_event);
}

pub(super) fn run_send_conversation_turn_effect(
    model: &mut Model,
    runtime_coordinator: &mut impl RuntimeCoordinator,
    request: ConversationTurnRequest,
) {
    match runtime_coordinator
        .dispatch_runtime_command(RuntimeCommand::submit_conversation_turn(request))
    {
        Ok(RuntimeCommandReceipt::ConversationStarted { activity_label }) => {
            model.show_stream_activity(activity_label)
        }
        Ok(_) => {}
        Err(message) => model.show_transient_status_notice(&message),
    }
}
