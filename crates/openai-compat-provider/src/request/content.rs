use provider_protocol::{
    ContentBlock, ImageDetail, ProviderError, ToolCall, ToolDefinition, visible_text_from_blocks,
};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AssistantProjection {
    pub(super) full_message: Value,
    pub(super) fragment_message: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolResultMessageProjection {
    pub(super) tool_message: Value,
    pub(super) image_parts: Vec<Value>,
}

pub(super) fn non_assistant_visible_text(blocks: &[ContentBlock]) -> Result<String, ProviderError> {
    if blocks.iter().any(|block| block.as_tool_call().is_some()) {
        return Err(ProviderError::Protocol(
            "tool call content is only valid on assistant messages".to_string(),
        ));
    }
    Ok(visible_text_from_blocks(blocks))
}

pub(super) fn system_message_value(content: &[ContentBlock]) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "system",
        "content": non_assistant_visible_text(content)?,
    }))
}

pub(super) fn user_message_value(content: &[ContentBlock]) -> Result<Value, ProviderError> {
    Ok(json!({
        "role": "user",
        "content": user_content_from_blocks(content)?,
    }))
}

pub(super) fn tool_result_message_projection(
    call_id: &str,
    content: &[ContentBlock],
) -> Result<ToolResultMessageProjection, ProviderError> {
    let text = non_assistant_visible_text(content)?;
    let image_parts = content
        .iter()
        .filter_map(tool_result_image_content_part)
        .collect::<Vec<_>>();
    let tool_content = if text.is_empty() && !image_parts.is_empty() {
        "(see attached image)".to_string()
    } else {
        text
    };
    let tool_message = json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": tool_content,
    });
    Ok(ToolResultMessageProjection {
        tool_message,
        image_parts,
    })
}

pub(super) fn tool_result_image_attachment_message(image_parts: Vec<Value>) -> Value {
    let mut user_content = Vec::with_capacity(image_parts.len() + 1);
    user_content.push(json!({
        "type": "text",
        "text": "Attached image(s) from tool result:",
    }));
    user_content.extend(image_parts);
    json!({
        "role": "user",
        "content": user_content,
    })
}

pub(super) fn assistant_projection(
    content: &[ContentBlock],
    reasoning: Option<&str>,
) -> Result<AssistantProjection, ProviderError> {
    let text = visible_text_from_blocks(content);
    let tool_calls = content
        .iter()
        .filter_map(ContentBlock::as_tool_call)
        .collect::<Vec<_>>();
    let has_tool_calls = !tool_calls.is_empty();

    let content_value = if text.is_empty() {
        Value::Null
    } else {
        Value::String(text)
    };
    let tool_calls_value = has_tool_calls.then(|| {
        Value::Array(
            tool_calls
                .into_iter()
                .map(openai_tool_call_from_call)
                .collect(),
        )
    });

    let mut full_message = assistant_message_map(content_value.clone(), tool_calls_value.as_ref());
    if let Some(reasoning) = reasoning
        && has_tool_calls
    {
        full_message.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.to_string()),
        );
    }

    let fragment_message = if reasoning.is_some() && has_tool_calls {
        Some(Value::Object(assistant_message_map(
            content_value,
            tool_calls_value.as_ref(),
        )))
    } else {
        None
    };

    Ok(AssistantProjection {
        full_message: Value::Object(full_message),
        fragment_message,
    })
}

pub(super) fn openai_tool_from_definition(definition: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": definition.name,
            "description": definition.description,
            "parameters": definition.input_schema,
        }
    })
}

fn assistant_message_map(content: Value, tool_calls: Option<&Value>) -> Map<String, Value> {
    let mut message = Map::new();
    message.insert("role".to_string(), Value::String("assistant".to_string()));
    message.insert("content".to_string(), content);
    if let Some(tool_calls) = tool_calls {
        message.insert("tool_calls".to_string(), tool_calls.clone());
    }
    message
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
            detail,
            ..
        } => Ok(Some(openai_image_url_content_part(
            mime_type,
            data_base64,
            *detail,
        ))),
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

fn tool_result_image_content_part(block: &ContentBlock) -> Option<Value> {
    let ContentBlock::Image {
        data_base64,
        mime_type,
        detail,
        ..
    } = block
    else {
        return None;
    };
    Some(openai_image_url_content_part(
        mime_type,
        data_base64,
        *detail,
    ))
}

fn openai_image_url_content_part(
    mime_type: &str,
    data_base64: &str,
    detail: Option<ImageDetail>,
) -> Value {
    let mut image_url = Map::new();
    image_url.insert(
        "url".to_string(),
        Value::String(data_uri(mime_type, data_base64)),
    );
    if let Some(detail) = chat_image_detail(detail) {
        image_url.insert("detail".to_string(), Value::String(detail.to_string()));
    }

    json!({
        "type": "image_url",
        "image_url": image_url,
    })
}

fn chat_image_detail(detail: Option<ImageDetail>) -> Option<&'static str> {
    match detail? {
        ImageDetail::Auto => Some("auto"),
        ImageDetail::Low => Some("low"),
        ImageDetail::High => Some("high"),
        ImageDetail::Original => None,
    }
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
