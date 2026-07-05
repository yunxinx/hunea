//! Context budget helpers for prepared turns.

use std::cmp::Reverse;

use openai_compat_provider::{
    OpenAiRequestFormat, prompt_request_projection_from_parts_for_format,
};
use provider_protocol::{ConversationItem, ToolDefinition};
use runtime_domain::{
    context_budget::{
        ContextBudgetSnapshot, ContextSegment, ContextTokenLimit, ContextWindowUsage, SegmentKind,
        context_window_usage,
    },
    prompt_assembly::{PromptPreludeSnapshot, PromptSourceKind},
    provider::ProviderKind,
    session::ContextBudgetProjectionErrorKind,
    token_count::{ESTIMATED_IMAGE_TOKENS, TokenEncoding},
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
    prompt_prelude: Option<&'a PromptPreludeSnapshot>,
    tool_definitions: &'a [ToolDefinition],
    context_limit: ContextTokenLimit,
    upstream_context_tokens: Option<usize>,
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
            prompt_prelude: None,
            tool_definitions,
            context_limit,
            upstream_context_tokens: None,
        }
    }

    /// `with_prompt_prelude` 为 system prompt 细分提供原始 prompt prelude 语义。
    #[must_use]
    pub fn with_prompt_prelude(
        mut self,
        prompt_prelude: Option<&'a PromptPreludeSnapshot>,
    ) -> Self {
        self.prompt_prelude = prompt_prelude;
        self
    }

    /// `with_upstream_context_tokens` 使用 provider 返回的总 token 数校准展示总量。
    #[must_use]
    pub fn with_upstream_context_tokens(mut self, upstream_context_tokens: Option<usize>) -> Self {
        self.upstream_context_tokens = upstream_context_tokens;
        self
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
        .with_prompt_prelude(request.prompt_prelude())
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
            prompt_request_projection_from_parts_for_format(
                OpenAiRequestFormat::ChatCompletions,
                probe.items,
                probe.tool_definitions,
            )
            .map_err(ContextBudgetError::projection)?
        }
        ProviderKind::OpenAiResponses => prompt_request_projection_from_parts_for_format(
            OpenAiRequestFormat::Responses,
            probe.items,
            probe.tool_definitions,
        )
        .map_err(ContextBudgetError::projection)?,
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
        .serialized_item_texts()
        .map_err(ContextBudgetError::projection)?;
    let tools_text = projection
        .serialized_tools_text()
        .map_err(ContextBudgetError::projection)?;

    let mut segments = Vec::with_capacity(probe.items.len() + usize::from(tools_text.is_some()));
    for (item, projection_text) in probe.items.iter().zip(message_texts.iter()) {
        if should_cancel() {
            return Ok(None);
        }
        segments.extend(context_segments_for_item(
            item,
            projection_text,
            probe.prompt_prelude,
            *token_encoding,
        ));
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
    mut segments: Vec<ContextSegment>,
) -> ContextBudgetSnapshot {
    let total_estimated_tokens = match probe.upstream_context_tokens {
        Some(upstream_context_tokens) => {
            scale_segments_to_total(&mut segments, upstream_context_tokens);
            upstream_context_tokens
        }
        None => segments
            .iter()
            .map(|segment| segment.estimated_tokens)
            .sum(),
    };
    let usage: ContextWindowUsage =
        context_window_usage(total_estimated_tokens, probe.context_limit);

    ContextBudgetSnapshot {
        model_id: probe.model_id.to_string(),
        segments,
        total_estimated_tokens,
        usage,
    }
}

fn scale_segments_to_total(segments: &mut [ContextSegment], total_tokens: usize) {
    if segments.is_empty() {
        return;
    }

    let weights = segments
        .iter()
        .map(|segment| segment.estimated_tokens)
        .collect::<Vec<_>>();
    for (segment, estimated_tokens) in segments
        .iter_mut()
        .zip(distribute_tokens_by_weight(total_tokens, &weights))
    {
        segment.estimated_tokens = estimated_tokens;
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

fn context_segments_for_item(
    item: &ConversationItem,
    projection_text: &str,
    prompt_prelude: Option<&PromptPreludeSnapshot>,
    token_encoding: TokenEncoding,
) -> Vec<ContextSegment> {
    if let Some(segments) =
        split_prompt_prelude_system_segment(item, projection_text, prompt_prelude, token_encoding)
    {
        return segments;
    }

    vec![ContextSegment {
        kind: segment_kind(item),
        estimated_tokens: estimate_projected_item_tokens(item, projection_text, token_encoding),
    }]
}

fn estimate_projected_item_tokens(
    item: &ConversationItem,
    projection_text: &str,
    token_encoding: TokenEncoding,
) -> usize {
    let Some(image_blocks) = image_blocks_for_item(item) else {
        return token_encoding.estimate_text(projection_text);
    };

    let mut estimation_text = projection_text.to_string();
    for &(mime_type, data_base64) in &image_blocks {
        let data_uri = format!("data:{mime_type};base64,{data_base64}");
        let placeholder = format!("data:{mime_type};base64,[image]");
        estimation_text = estimation_text.replace(&data_uri, &placeholder);
    }

    token_encoding
        .estimate_text(&estimation_text)
        .saturating_add(image_blocks.len().saturating_mul(ESTIMATED_IMAGE_TOKENS))
}

fn image_blocks_for_item(item: &ConversationItem) -> Option<Vec<(&str, &str)>> {
    let blocks = match item {
        ConversationItem::Message { content, .. }
        | ConversationItem::ToolResult { content, .. } => content,
        ConversationItem::Reasoning { .. } => return None,
    };
    let images = blocks
        .iter()
        .filter_map(|block| match block {
            provider_protocol::ContentBlock::Image {
                mime_type,
                data_base64,
                ..
            } => Some((mime_type.as_str(), data_base64.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();

    (!images.is_empty()).then_some(images)
}

fn split_prompt_prelude_system_segment(
    item: &ConversationItem,
    projection_text: &str,
    prompt_prelude: Option<&PromptPreludeSnapshot>,
    token_encoding: TokenEncoding,
) -> Option<Vec<ContextSegment>> {
    let ConversationItem::Message {
        role: provider_protocol::Role::System,
        ..
    } = item
    else {
        return None;
    };
    let prompt_prelude = prompt_prelude?;
    let buckets = prompt_prelude_buckets(prompt_prelude, token_encoding);
    if buckets.is_empty() {
        return None;
    }
    if buckets.len() == 1 && buckets[0].kind == SegmentKind::System {
        return None;
    }

    let total_tokens = token_encoding.estimate_text(projection_text);
    let weights = buckets
        .iter()
        .map(|bucket| bucket.token_weight)
        .collect::<Vec<_>>();
    let distributed = distribute_tokens_by_weight(total_tokens, &weights);

    Some(
        buckets
            .into_iter()
            .zip(distributed)
            .filter_map(|(bucket, estimated_tokens)| {
                (estimated_tokens > 0).then_some(ContextSegment {
                    kind: bucket.kind,
                    estimated_tokens,
                })
            })
            .collect(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PromptPreludeBucket {
    kind: SegmentKind,
    token_weight: usize,
}

fn prompt_prelude_buckets(
    prompt_prelude: &PromptPreludeSnapshot,
    token_encoding: TokenEncoding,
) -> Vec<PromptPreludeBucket> {
    let mut system_sections = Vec::new();
    let mut skill_sections = Vec::new();

    for section in &prompt_prelude.sections {
        let body = section.body.trim();
        if body.is_empty() {
            continue;
        }
        if section.kind == PromptSourceKind::SkillDiscovery {
            skill_sections.push(body.to_string());
        } else {
            system_sections.push(body.to_string());
        }
    }

    let mut buckets = Vec::with_capacity(2);
    if !system_sections.is_empty() {
        let body = system_sections.join("\n\n");
        buckets.push(PromptPreludeBucket {
            kind: SegmentKind::System,
            token_weight: token_encoding.estimate_text(&serialized_system_message(&body)),
        });
    }
    if !skill_sections.is_empty() {
        let body = skill_sections.join("\n\n");
        buckets.push(PromptPreludeBucket {
            kind: SegmentKind::SkillDiscovery,
            token_weight: token_encoding.estimate_text(&serialized_system_message(&body)),
        });
    }
    buckets
}

fn serialized_system_message(body: &str) -> String {
    serde_json::json!({
        "role": "system",
        "content": body,
    })
    .to_string()
}

fn distribute_tokens_by_weight(total_tokens: usize, weights: &[usize]) -> Vec<usize> {
    if weights.is_empty() {
        return Vec::new();
    }
    if total_tokens == 0 {
        return vec![0; weights.len()];
    }

    let total_weight = weights.iter().sum::<usize>();
    if total_weight == 0 {
        let mut distributed = vec![0; weights.len()];
        distributed[0] = total_tokens;
        return distributed;
    }

    let mut distributed = vec![0; weights.len()];
    let mut remainders = Vec::with_capacity(weights.len());
    let mut assigned = 0usize;

    for (index, weight) in weights.iter().copied().enumerate() {
        let numerator = weight.saturating_mul(total_tokens);
        let floor = numerator / total_weight;
        let remainder = numerator % total_weight;
        distributed[index] = floor;
        assigned = assigned.saturating_add(floor);
        remainders.push((remainder, index, weight));
    }

    let mut leftover = total_tokens.saturating_sub(assigned);
    remainders.sort_by_key(|&(remainder, _, _)| Reverse(remainder));
    for (_, index, _) in remainders {
        if leftover == 0 {
            break;
        }
        distributed[index] = distributed[index].saturating_add(1);
        leftover -= 1;
    }

    distributed
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider_protocol::{
        ContentBlock, ConversationItem, Role, ToolCall, ToolDefinition as ProviderToolDefinition,
    };
    use runtime_domain::context_budget::SegmentKind;
    use runtime_domain::prompt_assembly::{
        PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
    };
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
    fn prompt_prelude_skill_discovery_uses_dedicated_context_segment() {
        let mut session = ProviderConversation::new();
        session.set_prompt_prelude(Some(PromptPreludeSnapshot {
            sections: vec![
                PromptPreludeSection {
                    reference_id: "core-system".to_string(),
                    kind: PromptSourceKind::CoreSystemPrompt,
                    title: "Core system prompt".to_string(),
                    origin: Some(PromptSourceOrigin::Builtin),
                    body: "keep it direct".to_string(),
                },
                PromptPreludeSection {
                    reference_id: "skill-discovery".to_string(),
                    kind: PromptSourceKind::SkillDiscovery,
                    title: "Skill discovery".to_string(),
                    origin: Some(PromptSourceOrigin::Project),
                    body: "<available_skills>code-review</available_skills>".to_string(),
                },
            ],
        }));

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
            snapshot
                .segments
                .iter()
                .map(|segment| segment.kind)
                .collect::<Vec<_>>(),
            vec![
                SegmentKind::System,
                SegmentKind::SkillDiscovery,
                SegmentKind::UserMessage,
                SegmentKind::ToolDefinitions,
            ]
        );
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.estimated_tokens)
                .sum::<usize>(),
            snapshot.total_estimated_tokens,
            "split prompt prelude segments should still preserve the total token estimate"
        );
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
                detail: None,
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
    fn multimodal_image_context_budget_uses_semantic_image_estimate_not_base64_text_size() {
        let large_base64_payload = "a".repeat(96_000);
        let items = [ConversationItem::user(vec![ContentBlock::Image {
            data_base64: large_base64_payload.clone(),
            mime_type: "image/png".to_string(),
            uri: Some("assets/large.png".to_string()),
            detail: None,
        }])];

        for provider_kind in [
            ProviderKind::OpenAiCompatible,
            ProviderKind::OpenAiResponses,
        ] {
            let snapshot = build_context_budget_snapshot_with_cancellation(
                ContextBudgetProbe::new(
                    provider_kind,
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
                snapshot.segments[0].estimated_tokens >= ESTIMATED_IMAGE_TOKENS,
                "{provider_kind:?} image segment should include the shared image token estimate"
            );
            assert!(
                snapshot.segments[0].estimated_tokens
                    < estimate_text_tokens("gpt-4o", &large_base64_payload) / 2,
                "{provider_kind:?} image segment should not be dominated by base64 transfer bytes"
            );
        }
    }

    #[test]
    fn multimodal_tool_result_content_counts_more_than_visible_text_only() {
        let items = [
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "call-1",
                    "view_image",
                    r#"{"path":"assets/a.png"}"#,
                )],
            ),
            ConversationItem::tool_result(
                "call-1",
                vec![
                    ContentBlock::Text("loaded ".to_string()),
                    ContentBlock::Image {
                        data_base64: "iVBORw0KGgo=".to_string(),
                        mime_type: "image/png".to_string(),
                        uri: Some("assets/a.png".to_string()),
                        detail: None,
                    },
                ],
                false,
            ),
        ];
        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &[ProviderToolDefinition::new(
                    "view_image",
                    "View an image",
                    json!({"type": "object"}),
                )],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            ),
            || false,
        )
        .expect("context budget snapshot should build")
        .expect("never-cancelled snapshot should be present");

        assert!(
            snapshot.segments[1].estimated_tokens > estimate_text_tokens("gpt-4o", "loaded "),
            "multimodal tool result segment should include provider payload structure instead of only visible text"
        );
    }

    #[test]
    fn openai_responses_context_budget_projects_function_call_items() {
        let items = [
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new("call-1", "read", r#"{"path":"Cargo.toml"}"#)],
            ),
            ConversationItem::tool_result(
                "call-1",
                vec![ContentBlock::Text("workspace package".to_string())],
                false,
            ),
        ];

        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiResponses,
                "gpt-5-mini",
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
        .expect("responses context budget should project function call items")
        .expect("never-cancelled snapshot should be present");

        assert_eq!(snapshot.segments[0].kind, SegmentKind::AssistantMessage);
        assert!(
            snapshot.segments[0].estimated_tokens
                > estimate_text_tokens("gpt-5-mini", r#"{"path":"Cargo.toml"}"#),
            "Responses function call segment should count provider payload structure"
        );
    }

    #[test]
    fn upstream_context_tokens_scale_estimated_breakdown() {
        let items = [
            ConversationItem::text(Role::User, "first message"),
            ConversationItem::text(Role::Assistant, "second message"),
        ];
        let snapshot = build_context_budget_snapshot_with_cancellation(
            ContextBudgetProbe::new(
                ProviderKind::OpenAiCompatible,
                "gpt-4o",
                &items,
                &[],
                ContextTokenLimit::try_from(200_000).expect("fixture limit should be valid"),
            )
            .with_upstream_context_tokens(Some(1_000)),
            || false,
        )
        .expect("context budget snapshot should build")
        .expect("never-cancelled snapshot should be present");

        assert_eq!(snapshot.usage.used, 1_000);
        assert_eq!(snapshot.total_estimated_tokens, 1_000);
        assert_eq!(
            snapshot
                .segments
                .iter()
                .map(|segment| segment.estimated_tokens)
                .sum::<usize>(),
            1_000,
            "estimated category proportions should be mapped onto the upstream total"
        );
        assert!(
            snapshot
                .segments
                .iter()
                .all(|segment| segment.estimated_tokens > 0),
            "non-empty estimated categories should remain visible after scaling"
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
