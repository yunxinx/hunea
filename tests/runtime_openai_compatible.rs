use std::io::Cursor;

use lumos::runtime::openai_compatible::{
    CancellationToken, ChatCompletionMessage, ChatCompletionRequestBody,
    collect_chat_completion_stream, collect_chat_completion_stream_with_cancellation,
};

#[test]
fn chat_completion_stream_collects_delta_content_until_done() {
    let stream = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\" world\"}}]}\n\n",
        "data: [DONE]\n\n",
    );

    let content = collect_chat_completion_stream(Cursor::new(stream)).expect("stream should parse");

    assert_eq!(content, "Hello world");
}

#[test]
fn chat_completion_stream_reports_invalid_json_without_raw_payload_dump() {
    let stream = "data: {not-json}\n\n";

    let error = collect_chat_completion_stream(Cursor::new(stream))
        .expect_err("invalid stream JSON should fail");

    assert_eq!(error.to_string(), "invalid chat completion stream event");
}

#[test]
fn chat_completion_stream_honors_cancellation_before_collecting_content() {
    let token = CancellationToken::default();
    token.cancel();
    let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n";

    let error = collect_chat_completion_stream_with_cancellation(Cursor::new(stream), &token)
        .expect_err("cancelled stream should stop before collecting content");

    assert_eq!(error.to_string(), "chat completion cancelled");
}

#[test]
fn chat_completion_request_body_uses_streaming_chat_completions_shape() {
    let body = ChatCompletionRequestBody::new(
        "qwen3",
        vec![ChatCompletionMessage::user("hello".to_string())],
    );
    let value = serde_json::to_value(body).expect("request should serialize");

    assert_eq!(value["model"], "qwen3");
    assert_eq!(value["stream"], true);
    assert_eq!(value["messages"][0]["role"], "user");
    assert_eq!(value["messages"][0]["content"], "hello");
}
