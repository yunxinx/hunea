use std::collections::BTreeSet;

use provider_protocol::{ContentBlock, ConversationItem, ProviderError, Role};

pub(super) fn validate_openai_projection_items(
    items: &[ConversationItem],
) -> Result<(), ProviderError> {
    let mut pending_tool_call_ids = BTreeSet::new();
    let mut seen_tool_call_ids = BTreeSet::new();

    for (index, item) in items.iter().enumerate() {
        item.validate().map_err(|source| {
            ProviderError::Protocol(format!("invalid conversation item {index}: {source}"))
        })?;

        match item {
            ConversationItem::Message { role, content } => {
                ensure_no_pending_tool_calls(index, &pending_tool_call_ids)?;

                if *role == Role::Assistant {
                    for call in content.iter().filter_map(ContentBlock::as_tool_call) {
                        if !seen_tool_call_ids.insert(call.call_id.clone()) {
                            return Err(ProviderError::Protocol(format!(
                                "duplicate tool call `{}` at conversation item {index}",
                                call.call_id
                            )));
                        }
                        pending_tool_call_ids.insert(call.call_id.clone());
                    }
                }
            }
            ConversationItem::ToolResult { call_id, .. } => {
                if !pending_tool_call_ids.remove(call_id) {
                    return Err(ProviderError::Protocol(format!(
                        "tool result item {index} references unknown tool call `{call_id}`"
                    )));
                }
            }
            ConversationItem::Reasoning { .. } => {
                ensure_no_pending_tool_calls(index, &pending_tool_call_ids)?;
            }
        }
    }

    ensure_no_pending_tool_calls(items.len(), &pending_tool_call_ids)?;

    Ok(())
}

fn ensure_no_pending_tool_calls(
    index: usize,
    pending_tool_call_ids: &BTreeSet<String>,
) -> Result<(), ProviderError> {
    if pending_tool_call_ids.is_empty() {
        return Ok(());
    }

    Err(ProviderError::Protocol(format!(
        "unresolved tool calls before item {index}: {:?}",
        pending_tool_call_ids.iter().collect::<Vec<_>>()
    )))
}
