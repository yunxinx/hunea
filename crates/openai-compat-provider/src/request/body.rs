use provider_protocol::{PromptRequest, ProviderError};
use serde_json::{Value, json};

use super::projection::{PromptRequestProjection, prompt_request_projection};

pub(crate) fn chat_completion_request_body(
    request: &PromptRequest,
) -> Result<Value, ProviderError> {
    let projection = prompt_request_projection(request)?;
    let PromptRequestProjection {
        message_values,
        tools_value,
        ..
    } = projection;
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert("messages".to_string(), Value::Array(message_values));
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert(
        "stream_options".to_string(),
        json!({ "include_usage": true }),
    );

    if let Some(tools) = tools_value {
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
