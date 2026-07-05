use provider_protocol::{ConversationItem, FinishReason, StreamEvent, StreamEventSink};

use super::{
    OpenAiResponsesStreamState, OpenAiSseDecoder, OpenAiStreamState, ResponsesOutputItem,
    ResponsesOutputItemKind,
};

#[derive(Default)]
struct Events(Vec<StreamEvent>);

impl StreamEventSink for Events {
    fn emit(&mut self, event: StreamEvent) {
        self.0.push(event);
    }
}

fn assistant_item(completion: &provider_protocol::PromptCompletion) -> &ConversationItem {
    completion
        .items
        .iter()
        .find(|item| item.role() == Some(provider_protocol::Role::Assistant))
        .expect("expected assistant message in completion items")
}

#[test]
fn sse_decoder_handles_split_frames() {
    let mut decoder = OpenAiSseDecoder::default();
    assert!(decoder.push(b"data: {\"a\"").unwrap().is_empty());
    assert_eq!(
        decoder.push(b":1}\n\ndata: [DONE]\n\n").unwrap(),
        vec!["{\"a\":1}", "[DONE]"]
    );
}

#[test]
fn sse_decoder_joins_multiline_data_at_event_boundary() {
    let mut decoder = OpenAiSseDecoder::default();

    assert!(decoder.push(b"data: first\n").unwrap().is_empty());
    assert_eq!(
        decoder.push(b"data: second\n\n").unwrap(),
        vec!["first\nsecond"]
    );
}

#[test]
fn sse_decoder_flushes_complete_event_at_stream_end() {
    let mut decoder = OpenAiSseDecoder::default();

    assert!(decoder.push(b"data: [DONE]\n").unwrap().is_empty());

    assert_eq!(decoder.finish().unwrap(), vec!["[DONE]"]);
}

#[test]
fn sse_decoder_ignores_keepalive_events() {
    let mut decoder = OpenAiSseDecoder::default();

    assert_eq!(
        decoder
            .push(b"event: keepalive\ndata: ignored\n\ndata: {\"ok\":true}\n\n")
            .unwrap(),
        vec!["{\"ok\":true}"]
    );
}

#[test]
fn responses_output_item_kind_preserves_known_and_unknown_values() {
    let function_call = serde_json::from_str::<ResponsesOutputItem>(
        r#"{"type":"function_call","call_id":"call_1","name":"read"}"#,
    )
    .expect("function_call output item should deserialize");
    assert_eq!(function_call.kind, ResponsesOutputItemKind::FunctionCall);

    let unknown = serde_json::from_str::<ResponsesOutputItem>(r#"{"type":"web_search_call"}"#)
        .expect("unknown output item should deserialize");
    assert_eq!(
        unknown.kind,
        ResponsesOutputItemKind::Other("web_search_call".to_string())
    );
}

#[test]
fn stream_state_aggregates_tool_call_arguments() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\""}}]}}]}"#,
                &mut events,
            )
            .unwrap();
    state
            .apply_data_frame(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"Cargo.toml\"}"}}]},"finish_reason":"tool_calls"}]}"#,
                &mut events,
            )
            .unwrap();

    let completion = state.finish(&mut events).unwrap();
    let call = assistant_item(&completion)
        .tool_calls()
        .next()
        .expect("expected tool call");
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, r#"{"path":"Cargo.toml"}"#);
}

#[test]
fn stream_state_omits_incomplete_tool_call_when_finish_reason_is_length() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\""}}]},"finish_reason":"length"}]}"#,
                &mut events,
            )
            .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert_eq!(completion.finish_reason, FinishReason::Length);
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.tool_calls().next().is_none())
    );
    assert!(!events.0.iter().any(|event| {
        matches!(event, StreamEvent::ToolCallCompleted { index, .. } if *index == 0)
    }));
}

#[test]
fn stream_state_waits_for_tool_call_id_before_started_event() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
        .apply_data_frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read"}}]}}]}"#,
            &mut events,
        )
        .unwrap();
    assert!(!events.0.iter().any(|event| {
        matches!(event, StreamEvent::ToolCallStarted { index, .. } if *index == 0)
    }));

    state
        .apply_data_frame(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1"}]}}]}"#,
            &mut events,
        )
        .unwrap();

    assert!(events.0.iter().any(|event| {
        matches!(
            event,
            StreamEvent::ToolCallStarted { index, call_id, name }
                if *index == 0 && call_id == "call_1" && name == "read"
        )
    }));
}

#[test]
fn stream_state_errors_when_tool_call_finishes_without_id() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
                &mut events,
            )
            .unwrap();

    let error = state
        .finish(&mut events)
        .expect_err("tool call id is required by provider protocol");

    assert!(
        error
            .to_string()
            .contains("tool call 0 completed without an id")
    );
}

#[test]
fn stream_state_preserves_usage_finish_reason_and_hidden_reasoning() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"choices":[{"delta":{"reasoning_content":"think","content":"answer"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}}"#,
                &mut events,
            )
            .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert_eq!(completion.finish_reason, FinishReason::Stop);
    assert_eq!(
        completion
            .usage
            .expect("usage should be captured")
            .total_tokens,
        Some(7)
    );
    assert_eq!(completion.items[1].text_content(), "answer");
    assert!(
        matches!(&completion.items[0], ConversationItem::Reasoning { content, .. } if content == "think")
    );
}

#[test]
fn responses_stream_state_aggregates_text_tool_call_and_usage() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":""}}"#,
                &mut events,
            )
            .unwrap();
    state
            .apply_data_frame(
                r#"{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\""}"#,
                &mut events,
            )
            .unwrap();
    state
            .apply_data_frame(
                r#"{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"Cargo.toml\"}"}"#,
                &mut events,
            )
            .unwrap();
    state
        .apply_data_frame(
            r#"{"type":"response.output_text.delta","output_index":1,"delta":"done"}"#,
            &mut events,
        )
        .unwrap();
    state
            .apply_data_frame(
                r#"{"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":10,"output_tokens":3,"total_tokens":13}}}"#,
                &mut events,
            )
            .unwrap();

    let completion = state.finish(&mut events).unwrap();
    let assistant = assistant_item(&completion);
    let call = assistant
        .tool_calls()
        .next()
        .expect("expected response tool call");

    assert_eq!(assistant.text_content(), "done");
    assert_eq!(call.call_id, "call_1");
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments, r#"{"path":"Cargo.toml"}"#);
    assert_eq!(
        completion.usage.expect("usage should exist").total_tokens,
        Some(13)
    );
    assert!(events.0.iter().any(|event| {
        matches!(
            event,
            StreamEvent::ToolCallStarted { index, call_id, name }
                if *index == 0 && call_id == "call_1" && name == "read"
        )
    }));
}

#[test]
fn responses_stream_state_marks_completed_function_call_as_tool_call_finish() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}}"#,
                &mut events,
            )
            .unwrap();
    state
        .apply_data_frame(
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
            &mut events,
        )
        .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert_eq!(completion.finish_reason, FinishReason::ToolCalls);
}

#[test]
fn responses_stream_state_preserves_incomplete_function_call_finish_reason() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{\"path\""}}"#,
                &mut events,
            )
            .unwrap();
    state
        .apply_data_frame(
            r#"{"type":"response.incomplete","response":{"status":"incomplete"}}"#,
            &mut events,
        )
        .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert_eq!(completion.finish_reason, FinishReason::Length);
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.tool_calls().next().is_none())
    );
    assert!(!events.0.iter().any(|event| {
        matches!(event, StreamEvent::ToolCallCompleted { index, .. } if *index == 0)
    }));
}

#[test]
fn responses_stream_state_uses_final_message_item_when_text_deltas_are_absent() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"message","status":"completed","role":"assistant","content":[{"type":"output_text","text":"final answer","annotations":[]}]}}"#,
                &mut events,
            )
            .unwrap();
    state
        .apply_data_frame(
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
            &mut events,
        )
        .unwrap();

    let completion = state.finish(&mut events).unwrap();
    let assistant = assistant_item(&completion);

    assert_eq!(assistant.text_content(), "final answer");
    assert!(events.0.iter().any(|event| {
        matches!(event, StreamEvent::TextDelta(delta) if delta == "final answer")
    }));
}

#[test]
fn responses_stream_state_uses_final_reasoning_item_when_deltas_are_absent() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
            .apply_data_frame(
                r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[{"type":"summary_text","text":"final reasoning"}]}}"#,
                &mut events,
            )
            .unwrap();
    state
        .apply_data_frame(
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
            &mut events,
        )
        .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert!(matches!(
        &completion.items[0],
        ConversationItem::Reasoning { content, .. } if content == "final reasoning"
    ));
    assert!(events.0.iter().any(|event| {
        matches!(event, StreamEvent::ReasoningDelta(delta) if delta == "final reasoning")
    }));
}

#[test]
fn responses_stream_state_requires_terminal_event() {
    let mut state = OpenAiResponsesStreamState::default();
    let mut events = Events::default();
    state
        .apply_data_frame(
            r#"{"type":"response.output_text.delta","output_index":0,"delta":"partial"}"#,
            &mut events,
        )
        .unwrap();

    let error = state
        .finish(&mut events)
        .expect_err("responses stream must finish with a terminal event");

    assert!(
        error
            .to_string()
            .contains("Responses stream ended before a terminal response event")
    );
}

#[test]
fn stream_state_does_not_emit_empty_assistant_item_for_reasoning_only() {
    let mut state = OpenAiStreamState::new();
    let mut events = Events::default();
    state
        .apply_data_frame(
            r#"{"choices":[{"delta":{"reasoning_content":"think"},"finish_reason":"stop"}]}"#,
            &mut events,
        )
        .unwrap();

    let completion = state.finish(&mut events).unwrap();

    assert_eq!(completion.items.len(), 1);
    assert!(matches!(
        &completion.items[0],
        ConversationItem::Reasoning { content, .. } if content == "think"
    ));
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.role() != Some(provider_protocol::Role::Assistant))
    );
}
