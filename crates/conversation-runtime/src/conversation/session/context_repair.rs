use std::sync::Mutex;

use provider_protocol::{ContentBlock, ConversationItem, ToolCall};

#[derive(Debug, Default)]
pub(super) struct ProviderContextRepairLedger {
    unresolved_tool_calls: Vec<ToolCall>,
}

impl ProviderContextRepairLedger {
    pub(super) fn observe(&mut self, item: &ConversationItem) {
        for call in item.tool_calls() {
            self.unresolved_tool_calls.push(call.clone());
        }

        if let ConversationItem::ToolResult { call_id, .. } = item {
            self.resolve_tool_result(call_id);
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

    pub(super) fn take_repair_items(&mut self, content: &'static str) -> Vec<ConversationItem> {
        std::mem::take(&mut self.unresolved_tool_calls)
            .into_iter()
            .map(|call| {
                ConversationItem::tool_result(
                    call.call_id,
                    vec![ContentBlock::Text(content.to_string())],
                    true,
                )
            })
            .collect()
    }
}

pub(super) fn take_provider_context_repair_items(
    ledger: &Mutex<ProviderContextRepairLedger>,
    content: &'static str,
) -> Vec<ConversationItem> {
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take_repair_items(content)
}
