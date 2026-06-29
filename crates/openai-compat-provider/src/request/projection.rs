use std::borrow::Borrow;

use provider_protocol::{ConversationItem, PromptRequest, ProviderError, Role, ToolDefinition};
use serde_json::{Value, json};

use super::{
    content::{
        assistant_projection, openai_tool_from_definition, system_message_value,
        tool_result_message_value, user_message_value,
    },
    validation::validate_openai_projection_items,
};

/// `PromptRequestProjection` 保存按 OpenAI-compatible 请求格式投影后的 payload 片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequestProjection {
    pub(super) message_values: Vec<Value>,
    pub(super) message_fragments: Vec<MessageFragmentProjection>,
    pub(super) tools_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MessageFragmentProjection {
    SharedMessage(usize),
    Standalone(Value),
    Empty,
}

impl PromptRequestProjection {
    pub fn message_values(&self) -> &[Value] {
        &self.message_values
    }

    pub fn tools_value(&self) -> Option<&Value> {
        self.tools_value.as_ref()
    }

    pub fn serialized_message_texts(&self) -> Result<Vec<String>, ProviderError> {
        self.message_fragments
            .iter()
            .map(|fragment| match fragment {
                MessageFragmentProjection::SharedMessage(index) => {
                    serialize_json(self.projected_message_value(*index)?)
                }
                MessageFragmentProjection::Standalone(value) => serialize_json(value),
                MessageFragmentProjection::Empty => Ok(String::new()),
            })
            .collect()
    }

    pub fn serialized_tools_text(&self) -> Result<Option<String>, ProviderError> {
        self.tools_value.as_ref().map(serialize_json).transpose()
    }

    fn projected_message_value(&self, index: usize) -> Result<&Value, ProviderError> {
        self.message_values.get(index).ok_or_else(|| {
            ProviderError::Protocol(format!(
                "OpenAI request projection internal inconsistency: message fragment referenced missing projected message index {index}"
            ))
        })
    }
}

/// `prompt_request_projection` 将 prompt request 投影为 provider-side payload 片段。
pub fn prompt_request_projection(
    request: &PromptRequest,
) -> Result<PromptRequestProjection, ProviderError> {
    prompt_request_projection_from_parts(&request.items, &request.tools)
}

/// `prompt_request_projection_from_parts` 允许调用方直接用借用切片投影消息与工具定义。
pub fn prompt_request_projection_from_parts<Item>(
    items: &[Item],
    tools: &[ToolDefinition],
) -> Result<PromptRequestProjection, ProviderError>
where
    Item: Borrow<ConversationItem>,
{
    let (message_values, message_fragments) = project_items_to_messages_and_fragments(items)?;
    Ok(PromptRequestProjection {
        message_values,
        message_fragments,
        tools_value: project_tools_value(tools)?,
    })
}

fn project_items_to_messages_and_fragments<Item>(
    items: &[Item],
) -> Result<(Vec<Value>, Vec<MessageFragmentProjection>), ProviderError>
where
    Item: Borrow<ConversationItem>,
{
    validate_openai_projection_items(items)?;

    let mut messages = Vec::new();
    let mut message_fragments = vec![MessageFragmentProjection::Empty; items.len()];
    let mut pending_reasoning: Option<(usize, &str)> = None;
    let mut pending_tool_results: Vec<(usize, &ConversationItem)> = Vec::new();

    for (index, item) in items.iter().enumerate() {
        match item.borrow() {
            ConversationItem::Message {
                role: Role::System,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut message_fragments,
                )?;
                pending_reasoning = None;
                let value = system_message_value(content)?;
                message_fragments[index] = MessageFragmentProjection::SharedMessage(messages.len());
                messages.push(value);
            }
            ConversationItem::Message {
                role: Role::User,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut message_fragments,
                )?;
                pending_reasoning = None;
                let value = user_message_value(content)?;
                message_fragments[index] = MessageFragmentProjection::SharedMessage(messages.len());
                messages.push(value);
            }
            ConversationItem::Message {
                role: Role::Assistant,
                content,
            } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut message_fragments,
                )?;
                let has_tool_calls = content.iter().any(|block| block.as_tool_call().is_some());
                let reasoning = if has_tool_calls {
                    pending_reasoning.take()
                } else {
                    pending_reasoning = None;
                    None
                };
                if let Some((reasoning_index, reasoning_text)) = reasoning {
                    message_fragments[reasoning_index] =
                        MessageFragmentProjection::Standalone(json!({
                            "reasoning_content": reasoning_text
                        }));
                }
                if let Some((_, reasoning_text)) = reasoning {
                    let projection = assistant_projection(content, Some(reasoning_text))?;
                    if let Some(fragment_value) = projection.fragment_message {
                        message_fragments[index] =
                            MessageFragmentProjection::Standalone(fragment_value);
                    }
                    messages.push(projection.full_message);
                } else {
                    let projection = assistant_projection(content, None)?;
                    message_fragments[index] =
                        MessageFragmentProjection::SharedMessage(messages.len());
                    messages.push(projection.full_message);
                }
            }
            ConversationItem::ToolResult { .. } => {
                pending_tool_results.push((index, item.borrow()));
            }
            ConversationItem::Reasoning { content, .. } => {
                flush_tool_results(
                    &mut pending_tool_results,
                    &mut messages,
                    &mut message_fragments,
                )?;
                pending_reasoning = Some((index, content.as_str()));
            }
        }
    }

    flush_tool_results(
        &mut pending_tool_results,
        &mut messages,
        &mut message_fragments,
    )?;

    Ok((messages, message_fragments))
}

fn flush_tool_results(
    pending: &mut Vec<(usize, &ConversationItem)>,
    messages: &mut Vec<Value>,
    message_fragments: &mut [MessageFragmentProjection],
) -> Result<(), ProviderError> {
    for (index, item) in pending.drain(..) {
        if let ConversationItem::ToolResult {
            call_id, content, ..
        } = item
        {
            let value = tool_result_message_value(call_id, content)?;
            message_fragments[index] = MessageFragmentProjection::SharedMessage(messages.len());
            messages.push(value);
        }
    }
    Ok(())
}

fn project_tools_value(tools: &[ToolDefinition]) -> Result<Option<Value>, ProviderError> {
    if tools.is_empty() {
        return Ok(None);
    }
    Ok(Some(Value::Array(
        tools.iter().map(openai_tool_from_definition).collect(),
    )))
}

fn serialize_json(value: &Value) -> Result<String, ProviderError> {
    serde_json::to_string(value).map_err(|source| {
        ProviderError::Protocol(format!("serialize OpenAI request projection: {source}"))
    })
}
