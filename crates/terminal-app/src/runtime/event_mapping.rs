//! Agent fact 到 TUI runtime event 的转换。

use runtime_domain::{
    agent::{AgentEvent, AgentEventKind},
    session::RuntimeEvent,
};

pub(crate) fn runtime_event_from_agent_event(event: AgentEvent) -> RuntimeEvent {
    let target = event.target;
    match event.kind {
        AgentEventKind::SystemMessage { message } => RuntimeEvent::SystemMessage {
            target: Some(target),
            message,
        },
        AgentEventKind::Retrying { message } => RuntimeEvent::Retrying {
            target: Some(target),
            message,
        },
        AgentEventKind::OutputTokenEstimate { total_tokens } => RuntimeEvent::OutputTokenEstimate {
            target: Some(target),
            total_tokens,
        },
        AgentEventKind::InputTokenEstimate { total_tokens } => RuntimeEvent::InputTokenEstimate {
            target: Some(target),
            total_tokens,
        },
        AgentEventKind::Thinking { is_thinking } => RuntimeEvent::Thinking {
            target: Some(target),
            is_thinking,
        },
        AgentEventKind::AssistantDelta { content } => {
            RuntimeEvent::AssistantDelta { target, content }
        }
        AgentEventKind::ReasoningDelta { content } => {
            RuntimeEvent::ReasoningDelta { target, content }
        }
        AgentEventKind::ToolActivityStarted { activity } => {
            RuntimeEvent::ToolActivityStarted { target, activity }
        }
        AgentEventKind::ToolActivityUpdated { update } => {
            RuntimeEvent::ToolActivityUpdated { target, update }
        }
        AgentEventKind::TerminalUpdated { snapshot } => {
            RuntimeEvent::TerminalUpdated { target, snapshot }
        }
        AgentEventKind::PermissionRequested { request } => {
            RuntimeEvent::PermissionRequested { target, request }
        }
        AgentEventKind::PreparationWarning { message } | AgentEventKind::TurnFailed { message } => {
            RuntimeEvent::Failed {
                target: Some(target),
                message,
            }
        }
        AgentEventKind::TurnFinished {
            response,
            metrics,
            context_usage,
        } => RuntimeEvent::MessageFinished {
            target: Some(target),
            response,
            finish_reason: None,
            metrics,
            context_usage,
        },
        AgentEventKind::TurnInterrupted => RuntimeEvent::Interrupted {
            target: Some(target),
        },
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

#[cfg(test)]
mod tests {
    use runtime_domain::context_budget::{ContextTokenLimit, ContextWindowUsage};
    use runtime_domain::session::{ConversationResponse, RuntimeTarget};

    use super::*;
    use runtime_domain::agent::{AgentId, AgentTurnId};

    fn sample_usage() -> ContextWindowUsage {
        ContextWindowUsage {
            limit: ContextTokenLimit::new(128_000).expect("test limit should be non-zero"),
            used: 32_000,
        }
    }

    #[test]
    fn finished_event_carries_context_usage() {
        let usage = sample_usage();
        let event = runtime_event_from_agent_event(AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(1),
            target: RuntimeTarget::provider("openai", "gpt-4o-mini"),
            kind: AgentEventKind::TurnFinished {
                response: ConversationResponse::assistant_text("done"),
                metrics: None,
                context_usage: Some(usage),
            },
        });

        let RuntimeEvent::MessageFinished { context_usage, .. } = event else {
            panic!("finished conversation event should map to MessageFinished, got {event:?}");
        };
        assert_eq!(context_usage, Some(usage));
    }

    #[test]
    fn finished_event_without_usage_keeps_context_usage_hidden() {
        let event = runtime_event_from_agent_event(AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(1),
            target: RuntimeTarget::provider("openai", "gpt-4o-mini"),
            kind: AgentEventKind::TurnFinished {
                response: ConversationResponse::assistant_text("done"),
                metrics: None,
                context_usage: None,
            },
        });

        let RuntimeEvent::MessageFinished { context_usage, .. } = event else {
            panic!("finished conversation event should map to MessageFinished, got {event:?}");
        };
        assert_eq!(context_usage, None);
    }
}
