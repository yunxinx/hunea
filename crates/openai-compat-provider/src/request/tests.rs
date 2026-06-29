use provider_protocol::{
    ContentBlock, ConversationItem, PromptRequest, ProviderError, Role, ToolCall, ToolDefinition,
};
use serde_json::{Value, json};

use super::{
    body::chat_completion_request_body,
    content::{AssistantProjection, assistant_projection},
    projection::{
        MessageFragmentProjection, PromptRequestProjection, prompt_request_projection,
        prompt_request_projection_from_parts,
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
    let borrowed_projection = prompt_request_projection_from_parts(&request.items, &request.tools)
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
