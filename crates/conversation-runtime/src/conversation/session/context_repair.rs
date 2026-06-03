use std::sync::{Mutex, mpsc};

use provider_protocol::{Message, ToolCall, ToolResult};

use super::{ConversationDelta, ConversationWorkerEvent};

#[derive(Debug, Default)]
pub(super) struct ProviderContextRepairLedger {
    unresolved_tool_calls: Vec<ToolCall>,
}

impl ProviderContextRepairLedger {
    pub(super) fn observe(&mut self, message: &Message) {
        self.unresolved_tool_calls.extend(message.tool_calls());

        if let Some(tool_result) = message.first_tool_result() {
            self.resolve_tool_result(&tool_result.call_id);
        }
    }

    fn resolve_tool_result(&mut self, call_id: &str) {
        let Some(index) = self
            .unresolved_tool_calls
            .iter()
            .position(|call| call.call_id == call_id)
        else {
            return;
        };

        self.unresolved_tool_calls.remove(index);
    }

    pub(super) fn take_repair_messages(&mut self, content: &'static str) -> Vec<Message> {
        std::mem::take(&mut self.unresolved_tool_calls)
            .into_iter()
            .map(|call| {
                Message::tool_result(ToolResult::error(call.call_id, call.name, content, None))
            })
            .collect()
    }
}

pub(super) fn emit_provider_context_repair_messages(
    ledger: &Mutex<ProviderContextRepairLedger>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    content: &'static str,
) {
    let repair_messages = ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_repair_messages(content);

    for message in repair_messages {
        let _ = sender.send(ConversationWorkerEvent::Session(
            ConversationDelta::ProviderContextMessage { message },
        ));
    }
}
