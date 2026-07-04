use std::sync::mpsc;

use runtime_domain::session::ConversationEvent;

use crate::conversation::PersistedConversationItem;

use super::{ConversationDelta, ConversationWorker, ConversationWorkerEvent};

impl ConversationWorker {
    pub fn try_recv_event(&mut self) -> Option<ConversationEvent> {
        loop {
            let event = match self.receiver.as_ref()?.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.clear_runtime_state();
                    return Some(ConversationEvent::Failed {
                        message: "conversation request stopped before completion".to_string(),
                    });
                }
            };

            match event {
                ConversationWorkerEvent::Progress(event) => {
                    if event.is_terminal() {
                        self.clear_runtime_state();
                    }
                    return Some(event);
                }
                ConversationWorkerEvent::Session(event) => {
                    self.apply_session_event(event);
                }
                ConversationWorkerEvent::Finished {
                    response,
                    metrics,
                    upstream_context_tokens,
                } => {
                    self.upstream_context_tokens = upstream_context_tokens;
                    self.clear_runtime_state();
                    return Some(ConversationEvent::Finished { response, metrics });
                }
            }
        }
    }

    fn apply_session_event(&mut self, event: ConversationDelta) {
        match event {
            ConversationDelta::ProviderTurnStarted {
                session_id,
                user_entry_id: Some(user_entry_id),
            } => {
                if let Some(session_id) = session_id {
                    self.pending_session_id = Some(session_id);
                }
                self.pending_user_entry_id = Some(user_entry_id);
            }
            ConversationDelta::ProviderTurnStarted { session_id, .. } => {
                if let Some(session_id) = session_id {
                    self.pending_session_id = Some(session_id);
                }
                // retry 重放可能只表示“turn 已经持久化过”，不能清掉首次 entry id。
            }
            ConversationDelta::ProviderContextItem { entry_id, item } => {
                self.session_items
                    .push(PersistedConversationItem { entry_id, item });
            }
        }
    }

    fn clear_runtime_state(&mut self) {
        self.receiver = None;
        self.cancellation = None;
        self.target = None;
        self.pending_session_id = None;
        self.pending_user_entry_id = None;
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
        }
    }
}
