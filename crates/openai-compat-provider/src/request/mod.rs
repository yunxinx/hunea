use std::{borrow::Borrow, collections::BTreeSet};

use provider_protocol::{
    ContentBlock, ConversationItem, PromptRequest, ProviderError, Role, ToolCall, ToolDefinition,
    visible_text_from_blocks,
};
use serde_json::{Value, json};

/// `PromptRequestProjection` 保存按 OpenAI-compatible 请求格式投影后的 payload 片段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRequestProjection {
    message_values: Vec<Value>,
    message_fragments: Vec<MessageFragmentProjection>,
    tools_value: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MessageFragmentProjection {
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

pub(crate) fn chat_completion_request_body(
    request: &PromptRequest,
) -> Result<Value, ProviderError> {
    let projection = prompt_request_projection(request)?;
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert(
        "messages".to_string(),
        Value::Array(projection.message_values.clone()),
    );
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        json!({ "include_usage": true }),
    );

    if let Some(tools) = projection.tools_value().cloned() {
        body.insert("tools".to_string(), tools);
    }
    if let Some(temperature) = request.options.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_output_tokens) = request.options.max_output_tokens {
        body.insert(
            "max_completion_tokens".to_string(),
            json!(max_output_tokens),
        );
    }
    if let Some(top_p) = request.options.top_p {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(metadata) = request.options.metadata.clone() {
        body.insert("metadata".to_string(), metadata);
    }

    Ok(Value::Object(body))
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
                    let message_value = assistant_message_value(content, Some(reasoning_text))?;
                    let fragment_value = assistant_message_value(content, None)?;
                    message_fragments[index] =
                        MessageFragmentProjection::Standalone(fragment_value);
                    messages.push(message_value);
                } else {
                    let message_value = assistant_message_value(content, None)?;
                    message_fragments[index] =
                        MessageFragmentProjection::SharedMessage(messages.len());
                    messages.push(message_value);
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

fn validate_openai_projection_items<Item>(items: &[Item]) -> Result<(), ProviderError>
where
    Item: Borrow<ConversationItem>,
{
    let mut pending_tool_call_ids = BTreeSet::new();
    let mut seen_tool_call_ids = BTreeSet::new();

    for (index, item) in items.iter().enumerate() {
        let item = item.borrow();
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

fn non_assistant_visible_text(blocks: &[ContentBlock]) -> Result<String, ProviderError> {
    if blocks.iter().any(|block| block.as_tool_call().is_some()) {
        return Err(ProviderError::Protocol(
            "tool call content is only valid on assistant messages".to_string(),
        ));
    }
    Ok(visible_text_from_blocks(blocks))
}

fn system_message_value(content: &[ContentBlock]) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "system",
        "content": non_assistant_visible_text(content)?,
    }))
}

fn user_message_value(content: &[ContentBlock]) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "user",
        "content": user_content_from_blocks(content)?,
    }))
}

fn tool_result_message_value(
    call_id: &str,
    content: &[ContentBlock],
) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": non_assistant_visible_text(content)?,
    }))
}

fn assistant_message_value(
    content: &[ContentBlock],
    reasoning: Option<&str>,
) -> Result<Value, ProviderError> {
    let text = visible_text_from_blocks(content);
    let tool_calls = content
        .iter()
        .filter_map(ContentBlock::as_tool_call)
        .collect::<Vec<_>>();
    let has_tool_calls = !tool_calls.is_empty();

    let mut value = serde_json::Map::new();
    value.insert("role".to_string(), Value::String("assistant".to_string()));
    value.insert(
        "content".to_string(),
        if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        },
    );
    if let Some(reasoning) = reasoning
        && has_tool_calls
    {
        value.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.to_string()),
        );
    }
    if has_tool_calls {
        value.insert(
            "tool_calls".to_string(),
            Value::Array(
                tool_calls
                    .iter()
                    .map(|call| openai_tool_call_from_call(call))
                    .collect(),
            ),
        );
    }
    Ok(Value::Object(value))
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

fn user_content_from_blocks(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    let mut parts = Vec::new();
    for block in blocks {
        if let Some(part) = openai_user_content_part(block)? {
            parts.push(part);
        }
    }
    if parts.len() == 1
        && let Some(Value::Object(part)) = parts.first()
        && part.get("type").and_then(Value::as_str) == Some("text")
    {
        return Ok(part.get("text").cloned().unwrap_or_default());
    }
    Ok(Value::Array(parts))
}

fn openai_user_content_part(block: &ContentBlock) -> Result<Option<Value>, ProviderError> {
    match block {
        ContentBlock::Text(text) => Ok(Some(json!({ "type": "text", "text": text }))),
        ContentBlock::Image {
            data_base64,
            mime_type,
            ..
        } => Ok(Some(json!({
            "type": "image_url",
            "image_url": { "url": data_uri(mime_type, data_base64) },
        }))),
        ContentBlock::Audio {
            data_base64,
            mime_type,
            ..
        } => Ok(Some(json!({
            "type": "input_audio",
            "input_audio": {
                "data": data_base64,
                "format": audio_format(mime_type)?,
            },
        }))),
        ContentBlock::Document {
            data_base64,
            filename,
            ..
        } => Ok(Some(json!({
            "type": "file",
            "file": {
                "filename": filename.clone().unwrap_or_else(|| "document".to_string()),
                "file_data": data_base64,
            },
        }))),
        ContentBlock::ResourceLink {
            name,
            uri,
            mime_type,
            size,
        } => Ok(Some(json!({
            "type": "text",
            "text": resource_link_text(name, uri, mime_type.as_deref(), *size),
        }))),
        ContentBlock::ResourceText {
            uri,
            mime_type,
            text,
        } => Ok(Some(json!({
            "type": "text",
            "text": resource_text(uri, mime_type.as_deref(), text),
        }))),
        ContentBlock::ToolCall(_) => Err(ProviderError::Protocol(
            "tool call content is only valid on assistant messages".to_string(),
        )),
    }
}

fn openai_tool_from_definition(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": definition.name,
            "description": definition.description,
            "parameters": definition.input_schema,
        }
    })
}

fn openai_tool_call_from_call(call: &ToolCall) -> Value {
    json!({
        "id": call.call_id,
        "type": "function",
        "function": {
            "name": call.name,
            "arguments": call.arguments,
        }
    })
}

fn data_uri(mime_type: &str, data_base64: &str) -> String {
    format!("data:{mime_type};base64,{data_base64}")
}

fn audio_format(mime_type: &str) -> Result<&'static str, ProviderError> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/x-mp3" | "mp3" => Ok("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/wave" | "wav" => Ok("wav"),
        other => Err(ProviderError::Protocol(format!(
            "unsupported OpenAI chat audio input MIME type {other:?}; expected audio/mpeg or audio/wav"
        ))),
    }
}

fn resource_link_text(name: &str, uri: &str, mime_type: Option<&str>, size: Option<u64>) -> String {
    let mut text = format!("[Attached resource: {name}]({uri})");
    if let Some(mime_type) = mime_type {
        text.push_str(&format!(" ({mime_type})"));
    }
    if let Some(size) = size {
        text.push_str(&format!(" {size} bytes"));
    }
    text
}

fn resource_text(uri: &str, mime_type: Option<&str>, text: &str) -> String {
    match mime_type {
        Some(mime_type) => format!("[Attached resource: {uri} ({mime_type})]\n{text}"),
        None => format!("[Attached resource: {uri}]\n{text}"),
    }
}

#[cfg(test)]
mod tests {
    use provider_protocol::{
        ContentBlock, ConversationItem, PromptRequest, ProviderError, Role, ToolCall,
        ToolDefinition,
    };

    use super::{
        MessageFragmentProjection, PromptRequestProjection, chat_completion_request_body,
        prompt_request_projection, prompt_request_projection_from_parts,
    };

    #[test]
    fn multimodal_user_blocks_project_to_chat_completion_parts() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::user(vec![
                ContentBlock::Text("review ".to_string()),
                ContentBlock::Image {
                    data_base64: "iVBORw==".to_string(),
                    mime_type: "image/png".to_string(),
                    uri: None,
                },
            ])],
        );

        let body = chat_completion_request_body(&request).expect("request should build");
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/png;base64,iVBORw=="
        );
    }

    #[test]
    fn audio_and_file_blocks_use_chat_completion_provider_payloads() {
        let request = PromptRequest::new(
            "gpt-5-mini",
            vec![ConversationItem::user(vec![
                ContentBlock::Audio {
                    data_base64: "UklGRg==".to_string(),
                    mime_type: "audio/wav".to_string(),
                    uri: None,
                },
                ContentBlock::Document {
                    data_base64: "eyJrIjoidiJ9".to_string(),
                    mime_type: "application/json".to_string(),
                    filename: Some("payload.json".to_string()),
                    uri: None,
                },
            ])],
        );

        let body = chat_completion_request_body(&request).expect("request should build");
        let content = &body["messages"][0]["content"];

        assert_eq!(content[0]["type"], "input_audio");
        assert_eq!(content[0]["input_audio"]["data"], "UklGRg==");
        assert_eq!(content[0]["input_audio"]["format"], "wav");
        assert_eq!(content[1]["type"], "file");
        assert_eq!(content[1]["file"]["filename"], "payload.json");
        assert_eq!(content[1]["file"]["file_data"], "eyJrIjoidiJ9");
    }

    #[test]
    fn unsupported_audio_mime_type_is_a_protocol_error() {
        let request = PromptRequest::new(
            "gpt-5-mini",
            vec![ConversationItem::user(vec![ContentBlock::Audio {
                data_base64: "AAAA".to_string(),
                mime_type: "audio/flac".to_string(),
                uri: None,
            }])],
        );

        let error = chat_completion_request_body(&request).expect_err("flac is not a chat input");

        assert!(
            error
                .to_string()
                .contains("unsupported OpenAI chat audio input MIME type")
        );
    }

    #[test]
    fn max_output_tokens_projects_to_current_chat_completion_field() {
        let mut request = PromptRequest::new(
            "gpt-5-mini",
            vec![ConversationItem::text(Role::User, "summarize")],
        );
        request.options.max_output_tokens = Some(256);

        let body = chat_completion_request_body(&request).expect("request should build");
        let object = body.as_object().expect("request body should be an object");

        assert_eq!(object["max_completion_tokens"], 256);
        assert!(!object.contains_key("max_tokens"));
    }

    #[test]
    fn tool_definitions_project_to_function_tools() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "list files")],
        )
        .with_tools(vec![ToolDefinition::new(
            "list_dir",
            "List a workspace directory",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        )]);

        let body = chat_completion_request_body(&request).expect("request should build");

        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "list_dir");
        assert_eq!(
            body["tools"][0]["function"]["parameters"]["required"][0],
            "path"
        );
    }

    #[test]
    fn prompt_request_projection_keeps_openai_tool_wrapper() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "list files")],
        )
        .with_tools(vec![ToolDefinition::new(
            "list_dir",
            "List a workspace directory",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        )]);

        let projection =
            prompt_request_projection(&request).expect("projection should build successfully");
        let tools_text = projection
            .serialized_tools_text()
            .expect("tools text should serialize successfully")
            .expect("tools text should exist");

        assert!(tools_text.contains(r#""type":"function""#));
        assert!(tools_text.contains(r#""name":"list_dir""#));
        assert!(tools_text.contains(r#""required":["path"]"#));
    }

    #[test]
    fn prompt_request_projection_reuses_exact_provider_payload_fragments() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("thinking about it"),
                ConversationItem::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCall::new("c1", "bash", "{}")],
                ),
                ConversationItem::tool_result("c1", vec![ContentBlock::Text("done".into())], false),
            ],
        )
        .with_tools(vec![ToolDefinition::new(
            "list_dir",
            "List a workspace directory",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        )]);

        let projection =
            prompt_request_projection(&request).expect("projection should build successfully");
        let body = chat_completion_request_body(&request).expect("request should build");

        assert_eq!(
            projection.message_values(),
            body["messages"]
                .as_array()
                .expect("messages should remain an array"),
        );
        assert_eq!(projection.tools_value(), body.get("tools"));
    }

    #[test]
    fn borrowed_projection_matches_prompt_request_projection_for_messages_and_tools() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("thinking about it"),
                ConversationItem::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCall::new("c1", "bash", "{}")],
                ),
                ConversationItem::tool_result("c1", vec![ContentBlock::Text("done".into())], false),
            ],
        )
        .with_tools(vec![ToolDefinition::new(
            "list_dir",
            "List a workspace directory",
            serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"],
            }),
        )]);

        let owned_projection =
            prompt_request_projection(&request).expect("owned projection should build");
        let borrowed_projection =
            prompt_request_projection_from_parts(&request.items, &request.tools)
                .expect("borrowed projection should build");

        assert_eq!(
            borrowed_projection.message_values(),
            owned_projection.message_values()
        );
        assert_eq!(
            borrowed_projection.tools_value(),
            owned_projection.tools_value()
        );
        assert_eq!(
            borrowed_projection
                .serialized_message_texts()
                .expect("borrowed texts should serialize"),
            owned_projection
                .serialized_message_texts()
                .expect("owned texts should serialize")
        );
        assert_eq!(
            borrowed_projection
                .serialized_tools_text()
                .expect("borrowed tools should serialize"),
            owned_projection
                .serialized_tools_text()
                .expect("owned tools should serialize")
        );
    }

    #[test]
    fn reasoning_embedded_in_assistant_message_with_tool_calls() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("thinking about it"),
                ConversationItem::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCall::new("c1", "bash", "{}")],
                ),
                ConversationItem::tool_result("c1", vec![ContentBlock::Text("done".into())], false),
            ],
        );

        let body = chat_completion_request_body(&request).expect("request should build");
        let assistant = &body["messages"][0];

        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["reasoning_content"], "thinking about it");
        assert_eq!(assistant["tool_calls"][0]["function"]["name"], "bash");
    }

    #[test]
    fn prompt_request_projection_splits_reasoning_and_assistant_contributions() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("thinking about it"),
                ConversationItem::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCall::new("c1", "bash", "{}")],
                ),
                ConversationItem::tool_result("c1", vec![ContentBlock::Text("done".into())], false),
            ],
        );

        let projection =
            prompt_request_projection(&request).expect("projection should build successfully");

        let message_texts = projection
            .serialized_message_texts()
            .expect("projection texts should serialize successfully");

        assert_eq!(message_texts.len(), 3);
        assert!(message_texts[0].contains(r#""reasoning_content":"thinking about it""#));
        assert!(message_texts[1].contains(r#""tool_calls""#));
        assert!(!message_texts[1].contains(r#""reasoning_content""#));
        assert!(message_texts[2].contains(r#""tool_call_id":"c1""#));
    }

    #[test]
    fn reasoning_discarded_when_no_tool_calls() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("internal thought"),
                ConversationItem::text(Role::Assistant, "the answer"),
            ],
        );

        let body = chat_completion_request_body(&request).expect("request should build");
        let assistant = &body["messages"][0];

        assert_eq!(assistant["role"], "assistant");
        assert!(assistant.get("reasoning_content").is_none());
        assert_eq!(assistant["content"], "the answer");
    }

    #[test]
    fn tool_result_projects_as_tool_role_message() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::assistant_with_tool_calls(
                    String::new(),
                    vec![ToolCall::new("c1", "bash", "{}")],
                ),
                ConversationItem::tool_result(
                    "c1",
                    vec![ContentBlock::Text("output".into())],
                    false,
                ),
            ],
        );

        let body = chat_completion_request_body(&request).expect("request should build");

        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "c1");
        assert_eq!(body["messages"][1]["content"], "output");
    }

    #[test]
    fn system_tool_call_content_is_a_protocol_error() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::system(vec![ContentBlock::ToolCall(
                ToolCall::new("c1", "bash", "{}"),
            )])],
        );

        let error =
            chat_completion_request_body(&request).expect_err("system tool call is invalid");

        assert!(
            error
                .to_string()
                .contains("tool call content is only valid on assistant messages")
        );
    }

    #[test]
    fn tool_result_tool_call_content_is_a_protocol_error() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::tool_result(
                "c1",
                vec![ContentBlock::ToolCall(ToolCall::new(
                    "c2",
                    "bash",
                    "{}".to_string(),
                ))],
                false,
            )],
        );

        let error =
            chat_completion_request_body(&request).expect_err("tool result tool call is invalid");

        assert!(
            error
                .to_string()
                .contains("tool call content is only valid on assistant messages")
        );
    }

    #[test]
    fn orphan_reasoning_is_discarded_by_chat_projection() {
        let request = PromptRequest::new(
            "qwen3",
            vec![
                ConversationItem::reasoning("thinking"),
                ConversationItem::text(Role::User, "next"),
            ],
        );

        let body = chat_completion_request_body(&request).expect("request should build");

        assert_eq!(
            body["messages"].as_array().expect("messages array").len(),
            1
        );
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "next");
    }

    #[test]
    fn serialized_message_texts_returns_error_for_inconsistent_fragment_indices() {
        let projection = PromptRequestProjection {
            message_values: Vec::new(),
            message_fragments: vec![MessageFragmentProjection::SharedMessage(0)],
            tools_value: None,
        };

        let error = projection
            .serialized_message_texts()
            .expect_err("inconsistent fragment indices should return an error");

        assert!(
            matches!(error, ProviderError::Protocol(message) if message.contains("internal inconsistency"))
        );
    }

    #[test]
    fn duplicate_tool_call_id_is_a_protocol_error() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![
                    ToolCall::new("c1", "read", "{}"),
                    ToolCall::new("c1", "write", "{}"),
                ],
            )],
        );

        let error =
            chat_completion_request_body(&request).expect_err("duplicate call id should fail");

        assert!(error.to_string().contains("duplicate tool call"));
    }

    #[test]
    fn unknown_tool_result_is_a_protocol_error() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::tool_result(
                "missing",
                vec![ContentBlock::Text("output".into())],
                false,
            )],
        );

        let error =
            chat_completion_request_body(&request).expect_err("unknown tool result should fail");

        assert!(error.to_string().contains("unknown tool call"));
    }

    #[test]
    fn unresolved_tool_call_at_request_end_is_a_protocol_error() {
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new("c1", "bash", "{}")],
            )],
        );

        let error =
            chat_completion_request_body(&request).expect_err("unresolved tool call should fail");

        assert!(error.to_string().contains("unresolved tool calls"));
    }
}
