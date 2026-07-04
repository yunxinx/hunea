use provider_protocol::{
    ContentBlock, ConversationItem, ImageDetail, PromptCacheRetention, PromptRequest,
    ProviderError, Role, ToolCall, ToolDefinition,
};
use serde_json::{Value, json};

use super::{
    body::{chat_completion_request_body, responses_request_body},
    content::{AssistantProjection, assistant_projection},
    projection::{
        ItemFragmentProjection, OpenAiRequestFormat, PromptRequestProjection,
        prompt_request_projection, prompt_request_projection_for_format,
        prompt_request_projection_from_parts, prompt_request_projection_from_parts_for_format,
    },
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
                detail: None,
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
fn multimodal_user_image_detail_projects_to_chat_completion_image_url() {
    let request = PromptRequest::new(
        "qwen3",
        vec![ConversationItem::user(vec![ContentBlock::Image {
            data_base64: "iVBORw==".to_string(),
            mime_type: "image/png".to_string(),
            uri: None,
            detail: Some(ImageDetail::High),
        }])],
    );

    let body = chat_completion_request_body(&request).expect("request should build");
    let image_url = &body["messages"][0]["content"][0]["image_url"];

    assert_eq!(image_url["url"], "data:image/png;base64,iVBORw==");
    assert_eq!(image_url["detail"], "high");
}

#[test]
fn chat_completion_omits_original_image_detail_not_supported_by_image_url_parts() {
    let request = PromptRequest::new(
        "qwen3",
        vec![ConversationItem::user(vec![ContentBlock::Image {
            data_base64: "iVBORw==".to_string(),
            mime_type: "image/png".to_string(),
            uri: None,
            detail: Some(ImageDetail::Original),
        }])],
    );

    let body = chat_completion_request_body(&request).expect("request should build");
    let image_url = body["messages"][0]["content"][0]["image_url"]
        .as_object()
        .expect("image_url should be an object");

    assert_eq!(image_url["url"], "data:image/png;base64,iVBORw==");
    assert!(!image_url.contains_key("detail"));
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
fn prompt_cache_key_projects_to_openai_chat_completion_field() {
    let mut request = PromptRequest::new(
        "gpt-5-mini",
        vec![ConversationItem::text(Role::User, "summarize")],
    );
    request.options.prompt_cache_key = Some("session-123".to_string());

    let body = chat_completion_request_body(&request).expect("request should build");

    assert_eq!(body["prompt_cache_key"], "session-123");
}

#[test]
fn long_prompt_cache_retention_projects_to_chat_completion_field() {
    let mut request = PromptRequest::new(
        "fast-compatible-model",
        vec![ConversationItem::text(Role::User, "summarize")],
    );
    request.options.prompt_cache_key = Some("session-123".to_string());
    request.options.prompt_cache_retention = Some(PromptCacheRetention::Long24h);

    let body = chat_completion_request_body(&request).expect("request should build");

    assert_eq!(body["prompt_cache_key"], "session-123");
    assert_eq!(body["prompt_cache_retention"], "24h");
}

#[test]
fn responses_body_projects_cache_key_and_session_stable_fields() {
    let mut request = PromptRequest::new(
        "fast-compatible-model",
        vec![ConversationItem::text(Role::User, "summarize")],
    );
    request.options.prompt_cache_key = Some("session-123".to_string());
    request.options.prompt_cache_retention = Some(PromptCacheRetention::Long24h);

    let body = responses_request_body(&request).expect("request should build");

    assert_eq!(body["model"], "fast-compatible-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["prompt_cache_key"], "session-123");
    assert_eq!(body["prompt_cache_retention"], "24h");
    assert_eq!(body["input"][0]["role"], "user");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][0]["content"][0]["text"], "summarize");
}

#[test]
fn responses_body_projects_function_tools_without_chat_wrapper() {
    let request = PromptRequest::new(
        "fast-compatible-model",
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

    let body = responses_request_body(&request).expect("request should build");

    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["tools"][0]["name"], "list_dir");
    assert_eq!(body["tools"][0]["strict"], false);
    assert_eq!(body["tools"][0]["parameters"]["required"][0], "path");
    assert!(body["tools"][0].get("function").is_none());
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
}

#[test]
fn responses_body_projects_tool_call_history_as_response_items() {
    let request = PromptRequest::new(
        "fast-compatible-model",
        vec![
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new("c1", "read", r#"{"path":"Cargo.toml"}"#)],
            ),
            ConversationItem::tool_result(
                "c1",
                vec![ContentBlock::Text("workspace package".to_string())],
                false,
            ),
        ],
    );

    let body = responses_request_body(&request).expect("request should build");

    assert_eq!(body["input"][0]["type"], "function_call");
    assert_eq!(body["input"][0]["call_id"], "c1");
    assert_eq!(body["input"][0]["name"], "read");
    assert_eq!(body["input"][0]["arguments"], r#"{"path":"Cargo.toml"}"#);
    assert_eq!(body["input"][1]["type"], "function_call_output");
    assert_eq!(body["input"][1]["call_id"], "c1");
    assert_eq!(body["input"][1]["output"], "workspace package");
}

#[test]
fn responses_body_projects_tool_result_images_as_structured_output_items() {
    let request = PromptRequest::new(
        "fast-compatible-model",
        vec![
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "c1",
                    "view_image",
                    r#"{"path":"assets/a.png"}"#,
                )],
            ),
            ConversationItem::tool_result(
                "c1",
                vec![
                    ContentBlock::Text("image loaded".to_string()),
                    ContentBlock::Image {
                        data_base64: "iVBORw==".to_string(),
                        mime_type: "image/png".to_string(),
                        uri: Some("assets/a.png".to_string()),
                        detail: Some(ImageDetail::Original),
                    },
                ],
                false,
            ),
        ],
    );

    let body = responses_request_body(&request).expect("request should build");
    let output = &body["input"][1]["output"];

    assert_eq!(body["input"][1]["type"], "function_call_output");
    assert_eq!(body["input"][1]["call_id"], "c1");
    assert_eq!(output[0]["type"], "input_text");
    assert_eq!(output[0]["text"], "image loaded");
    assert_eq!(output[1]["type"], "input_image");
    assert_eq!(output[1]["detail"], "original");
    assert_eq!(output[1]["image_url"], "data:image/png;base64,iVBORw==");
}

#[test]
fn chat_completion_projects_tool_result_images_as_following_user_image_message() {
    let request = PromptRequest::new(
        "gpt-4o",
        vec![
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new(
                    "c1",
                    "view_image",
                    r#"{"path":"assets/a.png"}"#,
                )],
            ),
            ConversationItem::tool_result(
                "c1",
                vec![
                    ContentBlock::Text("image loaded".to_string()),
                    ContentBlock::Image {
                        data_base64: "iVBORw==".to_string(),
                        mime_type: "image/png".to_string(),
                        uri: Some("assets/a.png".to_string()),
                        detail: None,
                    },
                ],
                false,
            ),
        ],
    );

    let body = chat_completion_request_body(&request).expect("request should build");
    let messages = body["messages"].as_array().expect("messages array");

    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "c1");
    assert_eq!(messages[1]["content"], "image loaded");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "text");
    assert_eq!(
        messages[2]["content"][0]["text"],
        "Attached image(s) from tool result:"
    );
    assert_eq!(messages[2]["content"][1]["type"], "image_url");
    assert_eq!(
        messages[2]["content"][1]["image_url"]["url"],
        "data:image/png;base64,iVBORw=="
    );
}

#[test]
fn responses_request_body_reuses_projection_payload_and_tools() {
    let request = PromptRequest::new(
        "fast-compatible-model",
        vec![
            ConversationItem::assistant_with_tool_calls(
                String::new(),
                vec![ToolCall::new("c1", "read", r#"{"path":"Cargo.toml"}"#)],
            ),
            ConversationItem::tool_result(
                "c1",
                vec![ContentBlock::Text("workspace package".to_string())],
                false,
            ),
        ],
    )
    .with_tools(vec![ToolDefinition::new(
        "read",
        "Read a file",
        json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
        }),
    )]);

    let projection = prompt_request_projection_for_format(OpenAiRequestFormat::Responses, &request)
        .expect("responses projection should build");
    let body = responses_request_body(&request).expect("responses body should build");

    assert_eq!(
        projection.payload_values(),
        body["input"].as_array().expect("input should be an array")
    );
    assert_eq!(projection.tools_value(), body.get("tools"));
}

#[test]
fn responses_projection_rejects_unresolved_tool_call_at_request_end() {
    let request = PromptRequest::new(
        "fast-compatible-model",
        vec![ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new("c1", "read", "{}")],
        )],
    );

    let error = prompt_request_projection_for_format(OpenAiRequestFormat::Responses, &request)
        .expect_err("unresolved responses tool call should fail before request body is sent");

    assert!(error.to_string().contains("unresolved tool calls"));
}

#[test]
fn prompt_cache_key_is_clamped_to_openai_limit() {
    let mut request = PromptRequest::new(
        "gpt-5-mini",
        vec![ConversationItem::text(Role::User, "summarize")],
    );
    request.options.prompt_cache_key = Some("x".repeat(67));

    let body = chat_completion_request_body(&request).expect("request should build");

    assert_eq!(body["prompt_cache_key"], "x".repeat(64));
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
        projection.payload_values(),
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
    let borrowed_projection = prompt_request_projection_from_parts(&request.items, &request.tools)
        .expect("borrowed projection should build");

    assert_eq!(
        borrowed_projection.payload_values(),
        owned_projection.payload_values()
    );
    assert_eq!(
        borrowed_projection.tools_value(),
        owned_projection.tools_value()
    );
    assert_eq!(
        borrowed_projection
            .serialized_item_texts()
            .expect("borrowed texts should serialize"),
        owned_projection
            .serialized_item_texts()
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
        .serialized_item_texts()
        .expect("projection texts should serialize successfully");

    assert_eq!(message_texts.len(), 3);
    assert!(message_texts[0].contains(r#""reasoning_content":"thinking about it""#));
    assert!(message_texts[1].contains(r#""tool_calls""#));
    assert!(!message_texts[1].contains(r#""reasoning_content""#));
    assert!(message_texts[2].contains(r#""tool_call_id":"c1""#));
}

#[test]
fn assistant_projection_reuses_one_intermediate_shape_for_reasoning_tool_calls() {
    let projection = assistant_projection(
        &[ContentBlock::ToolCall(ToolCall::new("c1", "bash", "{}"))],
        Some("thinking about it"),
    )
    .expect("assistant projection should build");

    assert_eq!(
        projection,
        AssistantProjection {
            full_message: json!({
                "role": "assistant",
                "content": Value::Null,
                "reasoning_content": "thinking about it",
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            }),
            fragment_message: Some(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": "c1",
                    "type": "function",
                    "function": { "name": "bash", "arguments": "{}" }
                }]
            })),
        }
    );
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
            ConversationItem::tool_result("c1", vec![ContentBlock::Text("output".into())], false),
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

    let error = chat_completion_request_body(&request).expect_err("system tool call is invalid");

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
fn serialized_item_texts_returns_error_for_inconsistent_fragment_indices() {
    let projection = PromptRequestProjection {
        payload_values: Vec::new(),
        item_fragments: vec![ItemFragmentProjection::SharedPayload(0)],
        tools_value: None,
    };

    let error = projection
        .serialized_item_texts()
        .expect_err("inconsistent fragment indices should return an error");

    assert!(
        matches!(error, ProviderError::Protocol(message) if message.contains("internal inconsistency"))
    );
}

#[test]
fn chat_completion_keeps_tool_result_batch_before_attached_image_messages() {
    let items = vec![
        ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![
                ToolCall::new("call-1", "view_image", r#"{"path":"one.png"}"#),
                ToolCall::new("call-2", "view_image", r#"{"path":"two.png"}"#),
            ],
        ),
        ConversationItem::tool_result(
            "call-1",
            vec![ContentBlock::Image {
                data_base64: "one".to_string(),
                mime_type: "image/png".to_string(),
                uri: Some("one.png".to_string()),
                detail: None,
            }],
            false,
        ),
        ConversationItem::tool_result(
            "call-2",
            vec![ContentBlock::Image {
                data_base64: "two".to_string(),
                mime_type: "image/png".to_string(),
                uri: Some("two.png".to_string()),
                detail: None,
            }],
            false,
        ),
    ];

    let projection = prompt_request_projection_from_parts_for_format(
        OpenAiRequestFormat::ChatCompletions,
        &items,
        &[],
    )
    .expect("projection should accept paired tool results");

    let messages = projection.payload_values();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[3]["role"], "user");

    let user_content = messages[3]["content"]
        .as_array()
        .expect("attached images should use multipart user content");
    let image_count = user_content
        .iter()
        .filter(|part| part["type"] == "image_url")
        .count();
    assert_eq!(image_count, 2);
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

    let error = chat_completion_request_body(&request).expect_err("duplicate call id should fail");

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
