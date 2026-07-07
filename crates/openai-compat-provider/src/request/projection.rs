use provider_protocol::{
    ContentBlock, ConversationItem, PromptRequest, ProviderError, Role, ToolDefinition,
    visible_text_from_blocks,
};
use serde_json::{Value, json};

use super::{
    content::{
        assistant_projection, openai_tool_from_definition, system_message_value,
        tool_result_image_attachment_message, tool_result_message_projection, user_message_value,
    },
    validation::validate_openai_projection_items,
};

/// `PromptRequestProjection` 保存按 OpenAI-compatible 请求格式投影后的 payload 片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequestProjection {
    pub(super) payload_values: Vec<Value>,
    pub(super) item_fragments: Vec<ItemFragmentProjection>,
    pub(super) tools_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ItemFragmentProjection {
    SharedPayload(usize),
    SharedPayloadRange { start: usize, end: usize },
    Standalone(Value),
    Empty,
}

/// OpenAI-compatible 请求投影目标协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiRequestFormat {
    /// `/v1/chat/completions` message payload。
    ChatCompletions,
    /// `/v1/responses` input item payload。
    Responses,
}

impl PromptRequestProjection {
    pub fn payload_values(&self) -> &[Value] {
        &self.payload_values
    }

    pub fn tools_value(&self) -> Option<&Value> {
        self.tools_value.as_ref()
    }

    pub fn serialized_item_texts(&self) -> Result<Vec<String>, ProviderError> {
        self.item_fragments
            .iter()
            .map(|fragment| match fragment {
                ItemFragmentProjection::SharedPayload(index) => {
                    serialize_json(self.projected_payload_value(*index)?)
                }
                ItemFragmentProjection::SharedPayloadRange { start, end } => {
                    if start > end || *end > self.payload_values.len() {
                        return Err(ProviderError::Protocol(format!(
                            "OpenAI request projection internal inconsistency: item fragment referenced invalid projected payload range {start}..{end}"
                        )));
                    }
                    serialize_json(&Value::Array(self.payload_values[*start..*end].to_vec()))
                }
                ItemFragmentProjection::Standalone(value) => serialize_json(value),
                ItemFragmentProjection::Empty => Ok(String::new()),
            })
            .collect()
    }

    pub fn serialized_tools_text(&self) -> Result<Option<String>, ProviderError> {
        self.tools_value.as_ref().map(serialize_json).transpose()
    }

    fn projected_payload_value(&self, index: usize) -> Result<&Value, ProviderError> {
        self.payload_values.get(index).ok_or_else(|| {
            ProviderError::Protocol(format!(
                "OpenAI request projection internal inconsistency: item fragment referenced missing projected payload index {index}"
            ))
        })
    }
}

/// `prompt_request_projection` 将 prompt request 投影为 provider-side payload 片段。
pub fn prompt_request_projection(
    request: &PromptRequest,
) -> Result<PromptRequestProjection, ProviderError> {
    prompt_request_projection_for_format(OpenAiRequestFormat::ChatCompletions, request)
}

/// `prompt_request_projection_for_format` 按指定 OpenAI 协议投影请求片段。
pub fn prompt_request_projection_for_format(
    format: OpenAiRequestFormat,
    request: &PromptRequest,
) -> Result<PromptRequestProjection, ProviderError> {
    prompt_request_projection_from_parts_for_format(format, &request.items, &request.tools)
}

/// `prompt_request_projection_from_parts` 允许调用方直接用借用切片投影消息与工具定义。
pub fn prompt_request_projection_from_parts(
    items: &[ConversationItem],
    tools: &[ToolDefinition],
) -> Result<PromptRequestProjection, ProviderError> {
    prompt_request_projection_from_parts_for_format(
        OpenAiRequestFormat::ChatCompletions,
        items,
        tools,
    )
}

/// `prompt_request_projection_from_parts_for_format` 允许调用方直接用借用切片按协议投影。
pub fn prompt_request_projection_from_parts_for_format(
    format: OpenAiRequestFormat,
    items: &[ConversationItem],
    tools: &[ToolDefinition],
) -> Result<PromptRequestProjection, ProviderError> {
    let (payload_values, item_fragments) = match format {
        OpenAiRequestFormat::ChatCompletions => project_items_to_messages_and_fragments(items)?,
        OpenAiRequestFormat::Responses => project_items_to_responses_inputs_and_fragments(items)?,
    };
    Ok(PromptRequestProjection {
        payload_values,
        item_fragments,
        tools_value: project_tools_value(format, tools)?,
    })
}

fn project_items_to_messages_and_fragments(
    items: &[ConversationItem],
) -> Result<(Vec<Value>, Vec<ItemFragmentProjection>), ProviderError> {
    validate_openai_projection_items(items)?;

    let mut messages = Vec::new();
    let mut item_fragments = vec![ItemFragmentProjection::Empty; items.len()];
    let mut pending_reasoning: Option<(usize, &str)> = None;
    let mut pending_tool_results: Vec<(usize, &ConversationItem)> = Vec::new();

    for (index, item) in items.iter().enumerate() {
        match item {
            ConversationItem::Message {
                role: Role::System,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut item_fragments,
                )?;
                pending_reasoning = None;
                let value = system_message_value(content)?;
                item_fragments[index] = ItemFragmentProjection::SharedPayload(messages.len());
                messages.push(value);
            }
            ConversationItem::Message {
                role: Role::User,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut item_fragments,
                )?;
                pending_reasoning = None;
                let value = user_message_value(content)?;
                item_fragments[index] = ItemFragmentProjection::SharedPayload(messages.len());
                messages.push(value);
            }
            ConversationItem::Message {
                role: Role::Assistant,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut item_fragments,
                )?;
                let has_tool_calls = content.iter().any(|block| block.as_tool_call().is_some());
                let reasoning = if has_tool_calls {
                    pending_reasoning.take()
                } else {
                    pending_reasoning = None;
                    None
                };
                if let Some((reasoning_index, reasoning_text)) = reasoning {
                    item_fragments[reasoning_index] = ItemFragmentProjection::Standalone(json!({
                        "reasoning_content": reasoning_text
                    }));
                    let projection = assistant_projection(content, Some(reasoning_text))?;
                    if let Some(fragment_value) = projection.fragment_message {
                        item_fragments[index] = ItemFragmentProjection::Standalone(fragment_value);
                    }
                    messages.push(projection.full_message);
                } else {
                    let projection = assistant_projection(content, None)?;
                    item_fragments[index] = ItemFragmentProjection::SharedPayload(messages.len());
                    messages.push(projection.full_message);
                }
            }
            ConversationItem::ToolResult { .. } => {
                pending_tool_results.push((index, item));
            }
            ConversationItem::Reasoning { content, .. } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut item_fragments,
                )?;
                pending_reasoning = Some((index, content.as_str()));
            }
        }
    }

    flush_tool_results(
        &mut pending_tool_results,
        &mut messages,
        &mut item_fragments,
    )?;

    Ok((messages, item_fragments))
}

fn flush_tool_results(
    pending: &mut Vec<(usize, &ConversationItem)>,
    messages: &mut Vec<Value>,
    item_fragments: &mut [ItemFragmentProjection],
) -> Result<(), ProviderError> {
    if pending.is_empty() {
        return Ok(());
    }

    let mut projected_results = Vec::with_capacity(pending.len());
    let mut attached_image_parts = Vec::new();
    for (index, item) in pending.drain(..) {
        if let ConversationItem::ToolResult {
            call_id, content, ..
        } = item
        {
            let projection = tool_result_message_projection(call_id, content)?;
            attached_image_parts.extend(projection.image_parts.iter().cloned());
            projected_results.push((index, projection));
        }
    }

    let mut tool_message_indexes = Vec::with_capacity(projected_results.len());
    for (_, projection) in &projected_results {
        tool_message_indexes.push(messages.len());
        messages.push(projection.tool_message.clone());
    }

    let attached_image_message = (!attached_image_parts.is_empty())
        .then(|| tool_result_image_attachment_message(attached_image_parts));
    if let Some(message) = attached_image_message.as_ref() {
        messages.push(message.clone());
    }

    for ((index, projection), tool_message_index) in
        projected_results.into_iter().zip(tool_message_indexes)
    {
        item_fragments[index] = if projection.image_parts.is_empty() {
            ItemFragmentProjection::SharedPayload(tool_message_index)
        } else {
            ItemFragmentProjection::Standalone(Value::Array(vec![
                projection.tool_message,
                tool_result_image_attachment_message(projection.image_parts),
            ]))
        };
    }
    Ok(())
}

fn project_items_to_responses_inputs_and_fragments(
    items: &[ConversationItem],
) -> Result<(Vec<Value>, Vec<ItemFragmentProjection>), ProviderError> {
    validate_openai_projection_items(items)?;

    let mut input = Vec::new();
    let mut item_fragments = vec![ItemFragmentProjection::Empty; items.len()];

    for (index, item) in items.iter().enumerate() {
        let start = input.len();
        input.extend(responses_input_values_for_item(item)?);
        let end = input.len();
        item_fragments[index] = match end.saturating_sub(start) {
            0 => ItemFragmentProjection::Empty,
            1 => ItemFragmentProjection::SharedPayload(start),
            _ => ItemFragmentProjection::SharedPayloadRange { start, end },
        };
    }

    Ok((input, item_fragments))
}

fn responses_input_values_for_item(item: &ConversationItem) -> Result<Vec<Value>, ProviderError> {
    let mut values = Vec::new();
    match item {
        ConversationItem::Message {
            role: Role::System,
            content,
        } => values.push(json!({
            "role": "system",
            "content": visible_text_without_tool_calls(content)?,
        })),
        ConversationItem::Message {
            role: Role::User,
            content,
        } => values.push(json!({
            "role": "user",
            "content": responses_user_content(content)?,
        })),
        ConversationItem::Message {
            role: Role::Assistant,
            content,
        } => {
            let text = visible_text_from_blocks(content);
            if !text.is_empty() {
                values.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "text": text,
                        "annotations": []
                    }],
                }));
            }
            for tool_call in content.iter().filter_map(ContentBlock::as_tool_call) {
                values.push(json!({
                    "type": "function_call",
                    "call_id": tool_call.call_id,
                    "name": tool_call.name,
                    "arguments": tool_call.arguments,
                }));
            }
        }
        ConversationItem::ToolResult {
            call_id, content, ..
        } => values.push(json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": responses_tool_result_output(content)?,
        })),
        ConversationItem::Reasoning { .. } => {}
    }
    Ok(values)
}

fn responses_tool_result_output(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    if !blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { .. }))
    {
        return Ok(Value::String(visible_text_without_tool_calls(blocks)?));
    }

    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                parts.push(json!({ "type": "input_text", "text": text }));
            }
            ContentBlock::Image {
                data_base64,
                mime_type,
                detail,
                ..
            } => {
                parts.push(json!({
                    "type": "input_image",
                    "detail": responses_image_detail(*detail),
                    "image_url": format!("data:{mime_type};base64,{data_base64}"),
                }));
            }
            ContentBlock::Audio { .. }
            | ContentBlock::Document { .. }
            | ContentBlock::ResourceLink { .. }
            | ContentBlock::ResourceText { .. } => {
                let text = super::content::non_assistant_visible_text(std::slice::from_ref(block))?;
                if !text.is_empty() {
                    parts.push(json!({ "type": "input_text", "text": text }));
                }
            }
            ContentBlock::ToolCall(_) => {
                return Err(ProviderError::Protocol(
                    "tool call content is only valid on assistant messages".to_string(),
                ));
            }
        }
    }
    Ok(Value::Array(parts))
}

fn responses_user_content(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => {
                parts.push(json!({ "type": "input_text", "text": text }));
            }
            ContentBlock::Image {
                data_base64,
                mime_type,
                detail,
                ..
            } => {
                parts.push(json!({
                    "type": "input_image",
                    "detail": responses_image_detail(*detail),
                    "image_url": format!("data:{mime_type};base64,{data_base64}"),
                }));
            }
            ContentBlock::Audio { .. }
            | ContentBlock::Document { .. }
            | ContentBlock::ResourceLink { .. }
            | ContentBlock::ResourceText { .. } => {
                let text = super::content::non_assistant_visible_text(std::slice::from_ref(block))?;
                if !text.is_empty() {
                    parts.push(json!({ "type": "input_text", "text": text }));
                }
            }
            ContentBlock::ToolCall(_) => {
                return Err(ProviderError::Protocol(
                    "tool call content is only valid on assistant messages".to_string(),
                ));
            }
        }
    }
    Ok(Value::Array(parts))
}

fn responses_image_detail(detail: Option<provider_protocol::ImageDetail>) -> &'static str {
    detail
        .unwrap_or(provider_protocol::ImageDetail::Auto)
        .as_str()
}

fn visible_text_without_tool_calls(blocks: &[ContentBlock]) -> Result<String, ProviderError> {
    if blocks.iter().any(|block| block.as_tool_call().is_some()) {
        return Err(ProviderError::Protocol(
            "tool call content is only valid on assistant messages".to_string(),
        ));
    }
    Ok(visible_text_from_blocks(blocks))
}

fn project_tools_value(
    format: OpenAiRequestFormat,
    tools: &[ToolDefinition],
) -> Result<Option<Value>, ProviderError> {
    if tools.is_empty() {
        return Ok(None);
    }
    let values = match format {
        OpenAiRequestFormat::ChatCompletions => {
            tools.iter().map(openai_tool_from_definition).collect()
        }
        OpenAiRequestFormat::Responses => {
            tools.iter().map(responses_tool_from_definition).collect()
        }
    };
    Ok(Some(Value::Array(values)))
}

pub(super) fn responses_tool_from_definition(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": definition.name,
        "description": definition.description,
        "parameters": definition.input_schema,
        "strict": false,
    })
}

fn serialize_json(value: &Value) -> Result<String, ProviderError> {
    serde_json::to_string(value).map_err(|source| {
        ProviderError::Protocol(format!("serialize OpenAI request projection: {source}"))
    })
}
