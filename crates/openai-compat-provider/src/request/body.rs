use provider_protocol::{PromptRequest, ProviderError};
use serde_json::{Value, json};

use super::projection::{
    OpenAiRequestFormat, PromptRequestProjection, prompt_request_projection,
    prompt_request_projection_for_format,
};

const OPENAI_PROMPT_CACHE_KEY_MAX_CHARS: usize = 64;

pub(crate) fn chat_completion_request_body(
    request: &PromptRequest,
) -> Result<Value, ProviderError> {
    let projection = prompt_request_projection(request)?;
    let PromptRequestProjection {
        payload_values,
        tools_value,
        ..
    } = projection;
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert("messages".to_string(), Value::Array(payload_values));
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
    if let Some(prompt_cache_key) = request.options.prompt_cache_key.as_deref() {
        body.insert(
            "prompt_cache_key".to_string(),
            Value::String(clamp_prompt_cache_key(prompt_cache_key)),
        );
    }
    if let Some(prompt_cache_retention) = request.options.prompt_cache_retention {
        body.insert(
            "prompt_cache_retention".to_string(),
            Value::String(prompt_cache_retention.as_openai_value().to_string()),
        );
    }

    Ok(Value::Object(body))
}

pub(crate) fn responses_request_body(request: &PromptRequest) -> Result<Value, ProviderError> {
    let projection = prompt_request_projection_for_format(OpenAiRequestFormat::Responses, request)?;
    let PromptRequestProjection {
        payload_values,
        tools_value,
        ..
    } = projection;
    let mut body = serde_json::Map::new();
    body.insert("model".to_string(), Value::String(request.model.clone()));
    body.insert("input".to_string(), Value::Array(payload_values));
    body.insert("stream".to_string(), Value::Bool(true));
    body.insert("store".to_string(), Value::Bool(false));

    if let Some(tools) = tools_value {
        body.insert("tools".to_string(), tools);
        body.insert("tool_choice".to_string(), Value::String("auto".to_string()));
        body.insert("parallel_tool_calls".to_string(), Value::Bool(true));
    }
    if let Some(temperature) = request.options.temperature {
        body.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_output_tokens) = request.options.max_output_tokens {
        body.insert("max_output_tokens".to_string(), json!(max_output_tokens));
    }
    if let Some(top_p) = request.options.top_p {
        body.insert("top_p".to_string(), json!(top_p));
    }
    if let Some(metadata) = request.options.metadata.clone() {
        body.insert("metadata".to_string(), metadata);
    }
    if let Some(prompt_cache_key) = request.options.prompt_cache_key.as_deref() {
        body.insert(
            "prompt_cache_key".to_string(),
            Value::String(clamp_prompt_cache_key(prompt_cache_key)),
        );
    }
    if let Some(prompt_cache_retention) = request.options.prompt_cache_retention {
        body.insert(
            "prompt_cache_retention".to_string(),
            Value::String(prompt_cache_retention.as_openai_value().to_string()),
        );
    }

    Ok(Value::Object(body))
}

fn clamp_prompt_cache_key(key: &str) -> String {
    key.chars()
        .take(OPENAI_PROMPT_CACHE_KEY_MAX_CHARS)
        .collect()
}
