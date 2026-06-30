//! Context budget helpers for prepared turns.

use openai_compat_provider::prompt_request_projection_from_parts;
use provider_protocol::{ConversationItem, ToolDefinition};
use runtime_domain::{
    context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
        context_window_usage,
    },
    provider::ProviderKind,
    session::ContextBudgetProjectionErrorKind,
    token_count::TokenEncoding,
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
        failure: ContextBudgetProjectionFailure,
        #[source]
        source: provider_protocol::ProviderError,
    },
}

/// `ContextBudgetProjectionFailure` 提供可序列化、可分类的 projection 失败信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetProjectionFailure {
    pub kind: ContextBudgetProjectionErrorKind,
    pub status: Option<u16>,
    pub detail: Option<String>,
}

/// `ContextBudgetProbe` 描述一次 context budget 估算所需的 provider 输入。
#[derive(Debug)]
pub struct ContextBudgetProbe<'a> {
    provider_kind: ProviderKind,
    model_id: &'a str,
    items: &'a [ConversationItem],
    tool_definitions: &'a [ToolDefinition],
    context_limit: ContextTokenLimit,
}

impl<'a> ContextBudgetProbe<'a> {
    /// `new` 使用显式 provider 输入构造 context budget 估算请求。
    #[must_use]
    pub fn new(
        provider_kind: ProviderKind,
        model_id: &'a str,
        items: &'a [ConversationItem],
        tool_definitions: &'a [ToolDefinition],
        context_limit: ContextTokenLimit,
    ) -> Self {
        Self {
            provider_kind,
            model_id,
            items,
            tool_definitions,
            context_limit,
        }
    }

    /// `from_prepared_request` 使用已准备好的 turn request 构造估算请求。
    #[must_use]
    pub fn from_prepared_request(
        request: &'a PreparedConversationRequest,
        tool_definitions: &'a [ToolDefinition],
        context_limit: ContextTokenLimit,
    ) -> Self {
        Self::new(
            request.provider_kind(),
            request.model_id(),
            request.items(),
            tool_definitions,
            context_limit,
        )
    }
}

/// Uses the same provider-specific projection path as the real provider request and allows
/// cooperative cancellation between the expensive projection phases.
#[must_use = "building a snapshot can fail and the result must be handled"]
pub fn build_context_budget_snapshot_with_cancellation(
    probe: ContextBudgetProbe<'_>,
    should_cancel: impl Fn() -> bool,
) -> Result<Option<ContextBudgetSnapshot>, ContextBudgetError> {
    build_context_budget_snapshot_internal(probe, should_cancel)
}

fn build_context_budget_snapshot_internal(
    probe: ContextBudgetProbe<'_>,
    should_cancel: impl Fn() -> bool,
) -> Result<Option<ContextBudgetSnapshot>, ContextBudgetError> {
    if should_cancel() {
        return Ok(None);
    }

    let (projection, token_encoding) = project_probe(&probe)?;
    if should_cancel() {
        return Ok(None);
    }

    let Some(segments) = collect_segments(&probe, &projection, &token_encoding, &should_cancel)?
    else {
        return Ok(None);
    };

    Ok(Some(finish_snapshot(probe, segments)))
}

fn project_probe(
    probe: &ContextBudgetProbe<'_>,
) -> Result<
    (
        openai_compat_provider::PromptRequestProjection,
        TokenEncoding,
    ),
    ContextBudgetError,
> {
    let projection = match probe.provider_kind {
        ProviderKind::OpenAiCompatible | ProviderKind::OpenAi => {
            prompt_request_projection_from_parts(probe.items, probe.tool_definitions)
                .map_err(ContextBudgetError::projection)?
        }
        provider_kind => {
            return Err(ContextBudgetError::UnsupportedProvider { provider_kind });
        }
    };
    let token_encoding = TokenEncoding::for_model(probe.model_id);
    Ok((projection, token_encoding))
}

fn collect_segments(
    probe: &ContextBudgetProbe<'_>,
    projection: &openai_compat_provider::PromptRequestProjection,
    token_encoding: &TokenEncoding,
    should_cancel: impl Fn() -> bool,
) -> Result<Option<Vec<ContextSegment>>, ContextBudgetError> {
    let message_texts = projection
        .serialized_message_texts()
        .map_err(ContextBudgetError::projection)?;
    let tools_text = projection
        .serialized_tools_text()
        .map_err(ContextBudgetError::projection)?;

    let mut segments = Vec::with_capacity(probe.items.len() + usize::from(tools_text.is_some()));
    for (item, projection_text) in probe.items.iter().zip(message_texts.iter()) {
        if should_cancel() {
            return Ok(None);
        }
        let kind = segment_kind(item);
        segments.push(ContextSegment {
            kind,
            estimated_tokens: token_encoding.estimate_text(projection_text),
        });
    }

    if let Some(tools_text) = tools_text.as_deref() {
        if should_cancel() {
            return Ok(None);
        }
        segments.push(ContextSegment {
            kind: SegmentKind::ToolDefinitions,
            estimated_tokens: token_encoding.estimate_text(tools_text),
        });
    }

    Ok(Some(segments))
}

fn finish_snapshot(
    probe: ContextBudgetProbe<'_>,
    segments: Vec<ContextSegment>,
) -> ContextBudgetSnapshot {
    let total_estimated_tokens = segments
        .iter()
        .map(|segment| segment.estimated_tokens)
        .sum();
    let usage: ContextWindowUsage =
        context_window_usage(total_estimated_tokens, probe.context_limit);

    ContextBudgetSnapshot {
        model_id: probe.model_id.to_string(),
        segments,
        total_estimated_tokens,
        usage,
    }
}

impl ContextBudgetError {
    fn projection(source: provider_protocol::ProviderError) -> Self {
        Self::Projection {
            failure: projection_failure(&source),
            source,
        }
    }
}

fn projection_failure(source: &provider_protocol::ProviderError) -> ContextBudgetProjectionFailure {
    match source {
        provider_protocol::ProviderError::Protocol(detail) => ContextBudgetProjectionFailure {
            kind: ContextBudgetProjectionErrorKind::Protocol,
            status: None,
            detail: Some(detail.clone()),
        },
        provider_protocol::ProviderError::Transport(detail) => ContextBudgetProjectionFailure {
            kind: ContextBudgetProjectionErrorKind::Transport,
            status: None,
            detail: Some(detail.clone()),
        },
        provider_protocol::ProviderError::Provider { status, message } => {
            ContextBudgetProjectionFailure {
                kind: ContextBudgetProjectionErrorKind::Provider,
                status: *status,
                detail: Some(message.clone()),
            }
        }
    }
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

        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::from_prepared_request(
                &request,
                &[ProviderToolDefinition::new(
                    "read",
                    "Read a file",
                    json!({"type": "object"}),
                )],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect("context budget snapshot should build")
        .expect("never-cancelled snapshot should be present");

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
        assert_eq!(snapshot.usage.limit.get(), 200_000);
    }

    #[test]
    fn assistant_tool_calls_count_more_than_visible_text_only() {
        let items = [
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
        ];
        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &[ProviderToolDefinition::new(
                    "read",
                    "Read a file",
                    json!({"type": "object"}),
                )],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect("context budget snapshot should build")
        .expect("never-cancelled snapshot should be present");

        assert!(
            snapshot.segments[0].estimated_tokens > estimate_text_tokens("gpt-4o", "call it"),
            "assistant tool call segment should include provider payload overhead instead of only visible text"
        );
    }

    #[test]
    fn multimodal_user_content_counts_more_than_visible_text_only() {
        let items = [ConversationItem::user(vec![
            ContentBlock::Text("review ".to_string()),
            ContentBlock::Image {
                data_base64: "iVBORw0KGgo=".to_string(),
                mime_type: "image/png".to_string(),
                uri: None,
            },
        ])];
        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &[],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect("context budget snapshot should build")
        .expect("never-cancelled snapshot should be present");

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
        let items = [ConversationItem::text(Role::User, "hello")];
        let error = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::Anthropic,
                "claude-sonnet-4-5",
                &items,
                &[],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect_err("unsupported provider kinds should be explicit");

        assert!(matches!(
            error,
            ContextBudgetError::UnsupportedProvider {
                provider_kind: ProviderKind::Anthropic
            }
        ));
    }

    #[test]
    fn snapshot_segments_keep_provider_item_order_for_small_probe() {
        let items = [
            ConversationItem::text(Role::User, "first"),
            ConversationItem::text(Role::Assistant, "second"),
            ConversationItem::text(Role::User, "third"),
        ];

        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &[],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect("context budget snapshot should build for a small probe")
        .expect("never-cancelled snapshot should be present");

        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.kind)
                .collect::<Vec<_>>(),
            vec![
                SegmentKind::UserMessage,
                SegmentKind::AssistantMessage,
                SegmentKind::UserMessage,
            ]
        );
    }

    #[test]
    fn cancellation_closure_false_returns_snapshot() {
        let items = [
            ConversationItem::text(Role::System, "system rules"),
            ConversationItem::text(Role::User, "user input"),
            ConversationItem::text(Role::Assistant, "assistant reply"),
        ];
        let tool_definitions = [ProviderToolDefinition::new(
            "read",
            "Read a file",
            json!({"type": "object"}),
        )];
        let limit = ContextTokenLimit::try_from(32_000).expect("fixture limit should be valid");
        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &tool_definitions,
                limit,
            ),
            || false,
        )
        .expect("cancellable snapshot should build")
        .expect("never-cancelled snapshot should be present");

        assert_eq!(snapshot.usage.limit, limit);
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.kind)
                .collect::<Vec<_>>(),
            vec![
                SegmentKind::System,
                SegmentKind::UserMessage,
                SegmentKind::AssistantMessage,
                SegmentKind::ToolDefinitions,
            ]
        );
    }
}
