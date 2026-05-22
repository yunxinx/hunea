use mo_ai_core::{
    Message, MessageContent, MessageRole, PromptRequest, ProviderError, ToolCall, ToolDefinition,
};
use serde_json::{Value, json};

pub(crate) fn chat_completion_request_body(
    request: &PromptRequest,
) -> Result<Value, ProviderError> {
    let messages = request
        .messages
        .iter()
        .map(openai_message_from_message)
        .collect::<Result<Vec<_>, _>>()?;
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert("messages".to_string(), Value::Array(messages));
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        json!({ "include_usage": true }),
    );

    if !request.tools.is_empty() {
        body.insert(
            "tools".to_string(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(openai_tool_from_definition)
                    .collect(),
            ),
        );
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

fn openai_message_from_message(message: &Message) -> Result<Value, ProviderError> {
    match message.role {
        MessageRole::System => Ok(json!({
            "role": message.role.as_str(),
            "content": message.text_content(),
        })),
        MessageRole::User => Ok(json!({
            "role": message.role.as_str(),
            "content": user_content_from_blocks(&message.content)?,
        })),
        MessageRole::Assistant => assistant_message_from_message(message),
        MessageRole::Tool => tool_message_from_message(message),
    }
}

fn assistant_message_from_message(message: &Message) -> Result<Value, ProviderError> {
    let text = message.text_content();
    let tool_calls = message.tool_calls();
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
    if !tool_calls.is_empty() {
        value.insert(
            "tool_calls".to_string(),
            Value::Array(tool_calls.iter().map(openai_tool_call_from_call).collect()),
        );
    }
    Ok(Value::Object(value))
}

fn tool_message_from_message(message: &Message) -> Result<Value, ProviderError> {
    let Some(result) = message.first_tool_result() else {
        return Err(ProviderError::Protocol(
            "tool role message must contain a tool result".to_string(),
        ));
    };

    Ok(json!({
        "role": "tool",
        "tool_call_id": result.call_id,
        "content": result.content,
    }))
}

fn user_content_from_blocks(blocks: &[MessageContent]) -> Result<Value, ProviderError> {
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

fn openai_user_content_part(content: &MessageContent) -> Result<Option<Value>, ProviderError> {
    match content {
        MessageContent::Text(text) => Ok(Some(json!({ "type": "text", "text": text }))),
        MessageContent::Image {
            data_base64,
            mime_type,
            ..
        } => Ok(Some(json!({
            "type": "image_url",
            "image_url": { "url": data_uri(mime_type, data_base64) },
        }))),
        MessageContent::Audio {
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
        MessageContent::Document {
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
        MessageContent::ResourceLink {
            name,
            uri,
            mime_type,
            size,
        } => Ok(Some(json!({
            "type": "text",
            "text": resource_link_text(name, uri, mime_type.as_deref(), *size),
        }))),
        MessageContent::ResourceText {
            uri,
            mime_type,
            text,
        } => Ok(Some(json!({
            "type": "text",
            "text": resource_text(uri, mime_type.as_deref(), text),
        }))),
        MessageContent::Reasoning(_)
        | MessageContent::ToolCall(_)
        | MessageContent::ToolResult(_) => Ok(None),
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
            "arguments": call.arguments.to_string(),
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

fn resource_link_text(name: &str, uri: &str, mime_type: Option<&str>, size: Option<i64>) -> String {
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
    use mo_ai_core::{Message, MessageContent, MessageRole, PromptRequest, ToolDefinition};

    use super::chat_completion_request_body;

    #[test]
    fn multimodal_user_blocks_project_to_chat_completion_parts() {
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::new(
                MessageRole::User,
                vec![
                    MessageContent::Text("review ".to_string()),
                    MessageContent::Image {
                        data_base64: "iVBORw==".to_string(),
                        mime_type: "image/png".to_string(),
                        uri: None,
                    },
                ],
            )],
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
    fn audio_and_file_blocks_use_chat_completion_native_payloads() {
        let request = PromptRequest::new(
            "gpt-5-mini",
            vec![Message::new(
                MessageRole::User,
                vec![
                    MessageContent::Audio {
                        data_base64: "UklGRg==".to_string(),
                        mime_type: "audio/wav".to_string(),
                        uri: None,
                    },
                    MessageContent::Document {
                        data_base64: "eyJrIjoidiJ9".to_string(),
                        mime_type: "application/json".to_string(),
                        filename: Some("payload.json".to_string()),
                        uri: None,
                    },
                ],
            )],
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
            vec![Message::new(
                MessageRole::User,
                vec![MessageContent::Audio {
                    data_base64: "AAAA".to_string(),
                    mime_type: "audio/flac".to_string(),
                    uri: None,
                }],
            )],
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
            vec![Message::text(MessageRole::User, "summarize")],
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
            vec![Message::text(MessageRole::User, "list files")],
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
}
