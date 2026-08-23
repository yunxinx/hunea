use runtime_domain::session::ConversationEvent;

use super::ConversationWorkerEvent;
use crate::conversation::ConversationProgress;

pub(super) fn conversation_worker_event_from_progress(
    progress: ConversationProgress,
) -> Option<ConversationWorkerEvent> {
    match progress {
        ConversationProgress::SystemMessage { message } => Some(ConversationWorkerEvent::progress(
            ConversationEvent::SystemMessage { message },
        )),
        ConversationProgress::OutputTokens { total_tokens } => {
            Some(ConversationWorkerEvent::progress(
                ConversationEvent::OutputTokenEstimate { total_tokens },
            ))
        }
        ConversationProgress::InputTokens { total_tokens } => {
            Some(ConversationWorkerEvent::progress(
                ConversationEvent::InputTokenEstimate { total_tokens },
            ))
        }
        ConversationProgress::Thinking { is_thinking } => Some(ConversationWorkerEvent::progress(
            ConversationEvent::Thinking { is_thinking },
        )),
        ConversationProgress::AssistantDelta { content } => Some(
            ConversationWorkerEvent::progress(ConversationEvent::AssistantDelta { content }),
        ),
        ConversationProgress::ReasoningDelta { content } => Some(
            ConversationWorkerEvent::progress(ConversationEvent::ReasoningDelta { content }),
        ),
        ConversationProgress::ToolActivityStarted { activity } => Some(
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityStarted { activity }),
        ),
        ConversationProgress::ToolActivityUpdated { update } => Some(
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityUpdated { update }),
        ),
        ConversationProgress::TerminalUpdated { snapshot } => Some(
            ConversationWorkerEvent::progress(ConversationEvent::TerminalUpdated { snapshot }),
        ),
        ConversationProgress::ProviderTurnStarted
        | ConversationProgress::ProviderContextItem { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ConversationItem, Role};

    use super::conversation_worker_event_from_progress;
    use crate::conversation::ConversationProgress;

    #[test]
    fn session_only_progress_does_not_panic_when_seen_by_ui_mapper() {
        let turn_started = std::panic::catch_unwind(|| {
            let _ =
                conversation_worker_event_from_progress(ConversationProgress::ProviderTurnStarted);
        });
        assert!(turn_started.is_ok());

        let context_item = std::panic::catch_unwind(|| {
            let _ = conversation_worker_event_from_progress(
                ConversationProgress::ProviderContextItem {
                    item: ConversationItem::text(Role::Assistant, "persisted"),
                },
            );
        });
        assert!(context_item.is_ok());
    }
}
