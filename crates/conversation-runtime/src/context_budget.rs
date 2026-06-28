//! Context budget helpers for prepared turns.

use provider_protocol::ConversationItem;
use runtime_domain::context_budget::{ContextBudgetSnapshot, build_context_budget_snapshot};
use tool_loop_runtime::provider_tool_definitions_from_registry;
use tool_runtime::ToolExecutorRegistry;

use crate::PreparedConversationRequest;

/// Builds a context budget snapshot from a prepared turn request.
///
/// Uses `request.items()` in order (same as provider send). Optional `tool_definitions_text`
/// is appended as a single tools segment after all items.
pub fn context_budget_from_prepared_request(
    request: &PreparedConversationRequest,
    tool_definitions_text: Option<&str>,
    context_limit: Option<u32>,
) -> ContextBudgetSnapshot {
    build_context_budget_snapshot(
        request.model_id(),
        request.items(),
        tool_definitions_text,
        context_limit,
    )
}

/// Same as [`context_budget_from_prepared_request`] with explicit inputs (for tests or callers without full request).
pub fn context_budget_from_items(
    model_id: &str,
    items: &[ConversationItem],
    tool_definitions_text: Option<&str>,
    context_limit: Option<u32>,
) -> ContextBudgetSnapshot {
    build_context_budget_snapshot(model_id, items, tool_definitions_text, context_limit)
}

/// Serializes provider-visible tool definitions exactly as the provider path exports them.
pub fn context_budget_tool_definitions_text(
    executor: &ToolExecutorRegistry,
) -> Result<Option<String>, serde_json::Error> {
    let tool_definitions = provider_tool_definitions_from_registry(&executor.definitions());
    if tool_definitions.is_empty() {
        return Ok(None);
    }

    serde_json::to_string(&tool_definitions).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_protocol::{ConversationItem, Role};
    use runtime_domain::context_budget::SegmentKind;
    use runtime_domain::session::ConversationTurnRequest;
    use serde_json::json;
    use tool_runtime::{Tool, ToolDefinition, ToolExecutionFuture, ToolExecutorRegistry};

    use crate::conversation::PersistedConversationItem;
    use crate::{ProviderConversation, ProviderKind};

    #[test]
    fn prepare_turn_items_match_snapshot_segment_kinds_and_order() {
        let mut session = ProviderConversation::new();
        session.set_system_prompt(Some("You are helpful".to_string()));
        session.commit_turn_items([PersistedConversationItem {
            entry_id: None,
            item: ConversationItem::text(Role::User, "first"),
        }]);
        session.commit_turn_items([PersistedConversationItem {
            entry_id: None,
            item: ConversationItem::text(Role::Assistant, "answer"),
        }]);

        let request = session
            .prepare_turn(&ConversationTurnRequest::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                Some("http://127.0.0.1:1234/v1".to_string()),
                None,
                None,
                ConversationItem::text(Role::User, "follow up"),
            ))
            .expect("turn should prepare");

        let snapshot =
            context_budget_from_prepared_request(&request, Some("schema"), Some(200_000));

        assert_eq!(
            snapshot.segments.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![
                SegmentKind::System,
                SegmentKind::UserMessage,
                SegmentKind::AssistantMessage,
                SegmentKind::UserMessage,
                SegmentKind::ToolDefinitions,
            ]
        );
        assert!(matches!(
            snapshot.display,
            runtime_domain::context_budget::ContextLimitDisplay::Absolute { limit: 200_000, .. }
        ));
    }

    #[test]
    fn tool_definitions_text_matches_provider_visible_registry_export() {
        struct FakeTool;

        impl Tool for FakeTool {
            fn definition(&self) -> ToolDefinition {
                ToolDefinition::new("read")
                    .with_description("Read a file")
                    .with_input_schema(json!({
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" }
                        }
                    }))
            }

            fn execute<'a>(
                &'a self,
                _call: tool_runtime::ToolCall,
                _cancellation: &'a tokio_util::sync::CancellationToken,
            ) -> ToolExecutionFuture<'a> {
                Box::pin(async { tool_runtime::ToolResult::success("call-1", "") })
            }
        }

        let mut executor = ToolExecutorRegistry::new();
        executor.insert(FakeTool);

        let text = context_budget_tool_definitions_text(&executor)
            .expect("tool definitions should serialize")
            .expect("tool definitions should not be empty");

        assert!(text.contains("\"name\":\"read\""));
        assert!(text.contains("\"description\":\"Read a file\""));
    }
}
