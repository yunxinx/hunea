//! Context budget helpers for prepared turns.

use openai_compat_provider::prompt_request_projection;
use provider_protocol::{ConversationItem, PromptRequest, ToolDefinition};
use runtime_domain::{
    context_budget::{ContextBudgetSnapshot, ContextSegment, SegmentKind, context_limit_display},
    provider::ProviderKind,
    token_count::estimate_text_tokens,
};
use tool_loop_runtime::provider_tool_definitions_from_registry;
use tool_runtime::ToolExecutorRegistry;

use crate::PreparedConversationRequest;

/// `ContextBudgetError` 描述 context budget 投影失败。
#[derive(Debug, thiserror::Error)]
pub enum ContextBudgetError {
    #[error("context budget does not support provider kind {provider_kind}")]
    UnsupportedProvider { provider_kind: ProviderKind },
    #[error("context budget projection failed: {source}")]
    Projection {
        #[source]
        source: provider_protocol::ProviderError,
    },
}

/// Builds a context budget snapshot from a prepared turn request.
///
/// Uses the same provider-specific projection path as the real provider request.
pub fn context_budget_from_prepared_request(
    request: &PreparedConversationRequest,
    tool_definitions: &[ToolDefinition],
    context_limit: Option<u32>,
) -> Result<ContextBudgetSnapshot, ContextBudgetError> {
    context_budget_from_items(
        request.provider_kind(),
        request.model_id(),
        request.items(),
        tool_definitions,
        context_limit,
    )
}

/// Same as [`context_budget_from_prepared_request`] with explicit inputs.
pub fn context_budget_from_items(
    provider_kind: ProviderKind,
    model_id: &str,
    items: &[ConversationItem],
    tool_definitions: &[ToolDefinition],
    context_limit: Option<u32>,
) -> Result<ContextBudgetSnapshot, ContextBudgetError> {
    let prompt_request = PromptRequest::new(model_id.to_string(), items.to_vec())
        .with_tools(tool_definitions.to_vec());
    let projection = match provider_kind {
        ProviderKind::OpenAiCompatible | ProviderKind::OpenAi => {
            prompt_request_projection(&prompt_request)
                .map_err(|source| ContextBudgetError::Projection { source })?
        }
        provider_kind => {
            return Err(ContextBudgetError::UnsupportedProvider { provider_kind });
        }
    };

    let message_texts = projection
        .serialized_message_texts()
        .map_err(|source| ContextBudgetError::Projection { source })?;
    let tools_text = projection
        .serialized_tools_text()
        .map_err(|source| ContextBudgetError::Projection { source })?;

    let mut segments = Vec::with_capacity(items.len() + usize::from(tools_text.is_some()));
    for (stack_order, (item, projection_text)) in items.iter().zip(message_texts.iter()).enumerate()
    {
        segments.push(ContextSegment {
            kind: segment_kind(item),
            stack_order: u16::try_from(stack_order).unwrap_or(u16::MAX),
            estimated_tokens: estimate_text_tokens(model_id, projection_text),
            label: segment_kind(item).default_label().to_string(),
        });
    }

    if let Some(tools_text) = tools_text.as_deref() {
        segments.push(ContextSegment {
            kind: SegmentKind::ToolDefinitions,
            stack_order: u16::try_from(segments.len()).unwrap_or(u16::MAX),
            estimated_tokens: estimate_text_tokens(model_id, tools_text),
            label: SegmentKind::ToolDefinitions.default_label().to_string(),
        });
    }

    let total_estimated_tokens = segments
        .iter()
        .map(|segment| segment.estimated_tokens)
        .sum();
    let display = context_limit_display(total_estimated_tokens, context_limit);

    Ok(ContextBudgetSnapshot {
        model_id: model_id.to_string(),
        segments,
        total_estimated_tokens,
        context_limit,
        display,
    })
}

/// `context_budget_tool_definitions` 返回 provider-visible tool definitions。
pub fn context_budget_tool_definitions(executor: &ToolExecutorRegistry) -> Vec<ToolDefinition> {
    provider_tool_definitions_from_registry(&executor.definitions())
}

fn segment_kind(item: &ConversationItem) -> SegmentKind {
    match item {
        ConversationItem::Message { role, .. } => match role {
            provider_protocol::Role::System => SegmentKind::System,
            provider_protocol::Role::User => SegmentKind::UserMessage,
            provider_protocol::Role::Assistant => SegmentKind::AssistantMessage,
        },
        ConversationItem::ToolResult { .. } => SegmentKind::ToolResult,
        ConversationItem::Reasoning { .. } => SegmentKind::Reasoning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_protocol::{
        ContentBlock, ConversationItem, Role, ToolCall, ToolDefinition as ProviderToolDefinition,
    };
    use runtime_domain::context_budget::SegmentKind;
    use runtime_domain::session::ConversationTurnRequest;
    use runtime_domain::token_count::estimate_text_tokens;
    use serde_json::json;
    use tool_runtime::{
        Tool, ToolDefinition as RuntimeToolDefinition, ToolExecutionFuture, ToolExecutorRegistry,
    };

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

        let snapshot = context_budget_from_prepared_request(
            &request,
            &[ProviderToolDefinition::new(
                "read",
                "Read a file",
                json!({"type": "object"}),
            )],
            Some(200_000),
        )
        .expect("context budget snapshot should build");

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
    fn assistant_tool_calls_count_more_than_visible_text_only() {
        let snapshot = context_budget_from_items(
            ProviderKind::OpenAiCompatible,
            "gpt-4o",
            &[
                ConversationItem::assistant_with_tool_calls(
                    "call it".to_string(),
                    vec![ToolCall {
                        call_id: "call-1".to_string(),
                        name: "read".to_string(),
                        arguments: r#"{"path":"Cargo.toml"}"#.to_string(),
                    }],
                ),
                ConversationItem::tool_result(
                    "call-1",
                    vec![ContentBlock::Text("ok".to_string())],
                    false,
                ),
            ],
            &[ProviderToolDefinition::new(
                "read",
                "Read a file",
                json!({"type": "object"}),
            )],
            Some(200_000),
        )
        .expect("context budget snapshot should build");

        assert!(
            snapshot.segments[0].estimated_tokens > estimate_text_tokens("gpt-4o", "call it"),
            "assistant tool call segment should include provider payload overhead instead of only visible text"
        );
    }

    #[test]
    fn multimodal_user_content_counts_more_than_visible_text_only() {
        let snapshot = context_budget_from_items(
            ProviderKind::OpenAiCompatible,
            "gpt-4o",
            &[ConversationItem::user(vec![
                ContentBlock::Text("review ".to_string()),
                ContentBlock::Image {
                    data_base64: "iVBORw0KGgo=".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                },
            ])],
            &[],
            Some(200_000),
        )
        .expect("context budget snapshot should build");

        assert!(
            snapshot.segments[0].estimated_tokens > estimate_text_tokens("gpt-4o", "review "),
            "multimodal user segment should include provider payload structure instead of only visible text"
        );
    }

    #[test]
    fn tool_definitions_match_provider_visible_registry_export() {
        struct FakeTool;

        impl Tool for FakeTool {
            fn definition(&self) -> RuntimeToolDefinition {
                RuntimeToolDefinition::new("read")
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

        let tool_definitions = context_budget_tool_definitions(&executor);

        assert_eq!(tool_definitions.len(), 1);
        assert_eq!(tool_definitions[0].name, "read");
        assert_eq!(tool_definitions[0].description, "Read a file");
    }

    #[test]
    fn unsupported_provider_kind_returns_explicit_error() {
        let error = context_budget_from_items(
            ProviderKind::Anthropic,
            "claude-sonnet-4-5",
            &[ConversationItem::text(Role::User, "hello")],
            &[],
            Some(200_000),
        )
        .expect_err("unsupported provider kinds should be explicit");

        assert!(matches!(
            error,
            ContextBudgetError::UnsupportedProvider {
                provider_kind: ProviderKind::Anthropic
            }
        ));
    }
}
