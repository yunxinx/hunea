//! Agent fact 到 TUI runtime event 的转换。

use runtime_domain::{
    agent::{AgentEvent, AgentEventKind},
    session::RuntimeEvent,
};

pub(crate) fn runtime_event_from_main_agent_event(event: AgentEvent) -> Option<RuntimeEvent> {
    if event.agent_id != runtime_domain::agent::AgentId::MAIN {
        return None;
    }
    let target = event.target;
    match event.kind {
        AgentEventKind::SystemMessage { message } => Some(RuntimeEvent::SystemMessage {
            target: Some(target),
            message,
        }),
        AgentEventKind::Retrying { message } => Some(RuntimeEvent::Retrying {
            target: Some(target),
            message,
        }),
        AgentEventKind::OutputTokenEstimate { total_tokens } => {
            Some(RuntimeEvent::OutputTokenEstimate {
                target: Some(target),
                total_tokens,
            })
        }
        AgentEventKind::InputTokenEstimate { total_tokens } => {
            Some(RuntimeEvent::InputTokenEstimate {
                target: Some(target),
                total_tokens,
            })
        }
        AgentEventKind::Thinking { is_thinking } => Some(RuntimeEvent::Thinking {
            target: Some(target),
            is_thinking,
        }),
        AgentEventKind::AssistantDelta { content } => {
            Some(RuntimeEvent::AssistantDelta { target, content })
        }
        AgentEventKind::ReasoningDelta { content } => {
            Some(RuntimeEvent::ReasoningDelta { target, content })
        }
        AgentEventKind::ToolActivityStarted { activity } => {
            Some(RuntimeEvent::ToolActivityStarted { target, activity })
        }
        AgentEventKind::ToolActivityUpdated { update } => {
            Some(RuntimeEvent::ToolActivityUpdated { target, update })
        }
        AgentEventKind::TerminalUpdated { snapshot } => {
            Some(RuntimeEvent::TerminalUpdated { target, snapshot })
        }
        AgentEventKind::PermissionRequested { request } => {
            Some(RuntimeEvent::PermissionRequested { target, request })
        }
        AgentEventKind::PreparationWarning { message } | AgentEventKind::TurnFailed { message } => {
            Some(RuntimeEvent::Failed {
                target: Some(target),
                message,
            })
        }
        AgentEventKind::TurnFinished {
            response,
            metrics,
            context_usage,
        } => Some(RuntimeEvent::MessageFinished {
            target: Some(target),
            response,
            finish_reason: None,
            metrics,
            context_usage,
        }),
        AgentEventKind::TurnInterrupted => Some(RuntimeEvent::Interrupted {
            target: Some(target),
        }),
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
        let event = runtime_event_from_main_agent_event(AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(1),
            target: RuntimeTarget::provider("openai", "gpt-4o-mini"),
            kind: AgentEventKind::TurnFinished {
                response: ConversationResponse::assistant_text("done"),
                metrics: None,
                context_usage: Some(usage),
            },
        });

        let Some(RuntimeEvent::MessageFinished { context_usage, .. }) = event else {
            panic!("finished conversation event should map to MessageFinished");
        };
        assert_eq!(context_usage, Some(usage));
    }

    #[test]
    fn finished_event_without_usage_keeps_context_usage_hidden() {
        let event = runtime_event_from_main_agent_event(AgentEvent {
            agent_id: AgentId::MAIN,
            turn_id: AgentTurnId::new(1),
            target: RuntimeTarget::provider("openai", "gpt-4o-mini"),
            kind: AgentEventKind::TurnFinished {
                response: ConversationResponse::assistant_text("done"),
                metrics: None,
                context_usage: None,
            },
        });

        let Some(RuntimeEvent::MessageFinished { context_usage, .. }) = event else {
            panic!("finished conversation event should map to MessageFinished");
        };
        assert_eq!(context_usage, None);
    }

    #[test]
    fn non_main_event_is_dropped_instead_of_panicking_or_retargeting() {
        let event = runtime_event_from_main_agent_event(AgentEvent {
            agent_id: AgentId::new(2),
            turn_id: AgentTurnId::new(1),
            target: RuntimeTarget::provider("private", "model"),
            kind: AgentEventKind::TurnInterrupted,
        });

        assert!(event.is_none());
    }
}
