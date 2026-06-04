use std::{sync::mpsc, thread};

use runtime_domain::session::ConversationEvent;

use super::{ConversationDelta, ConversationWorkerEvent};
use crate::conversation::ConversationProgress;

pub(super) fn progress_sender_to_permission_sender(
    sender: mpsc::Sender<ConversationWorkerEvent>,
) -> mpsc::Sender<ConversationEvent> {
    let (permission_sender, permission_receiver) = mpsc::channel();
    thread::spawn(move || {
        while let Ok(event) = permission_receiver.recv() {
            let _ = sender.send(ConversationWorkerEvent::progress(event));
        }
    });
    permission_sender
}

pub(super) fn conversation_worker_event_from_progress(
    progress: ConversationProgress,
) -> ConversationWorkerEvent {
    match progress {
        ConversationProgress::SystemMessage { message } => {
            ConversationWorkerEvent::progress(ConversationEvent::SystemMessage { message })
        }
        ConversationProgress::ProviderTurnStarted => {
            ConversationWorkerEvent::Session(ConversationDelta::ProviderTurnStarted)
        }
        ConversationProgress::ProviderContextItem { item } => {
            ConversationWorkerEvent::Session(ConversationDelta::ProviderContextItem { item })
        }
        ConversationProgress::OutputTokens { total_tokens } => {
            ConversationWorkerEvent::progress(ConversationEvent::OutputTokenEstimate {
                total_tokens,
            })
        }
        ConversationProgress::InputTokens { total_tokens } => {
            ConversationWorkerEvent::progress(ConversationEvent::InputTokenEstimate {
                total_tokens,
            })
        }
        ConversationProgress::Thinking { is_thinking } => {
            ConversationWorkerEvent::progress(ConversationEvent::Thinking { is_thinking })
        }
        ConversationProgress::AssistantDelta { content } => {
            ConversationWorkerEvent::progress(ConversationEvent::AssistantDelta { content })
        }
        ConversationProgress::ReasoningDelta { content } => {
            ConversationWorkerEvent::progress(ConversationEvent::ReasoningDelta { content })
        }
        ConversationProgress::ToolActivityStarted { activity } => {
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityStarted { activity })
        }
        ConversationProgress::ToolActivityUpdated { update } => {
            ConversationWorkerEvent::progress(ConversationEvent::ToolActivityUpdated { update })
        }
        ConversationProgress::TerminalUpdated { snapshot } => {
            ConversationWorkerEvent::progress(ConversationEvent::TerminalUpdated { snapshot })
        }
        ConversationProgress::ManagedSearchToolAuthorization { tool } => {
            ConversationWorkerEvent::progress(ConversationEvent::ManagedSearchToolAuthorization {
                tool,
            })
        }
    }
}
