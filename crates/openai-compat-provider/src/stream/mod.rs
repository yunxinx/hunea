use std::collections::{BTreeMap, HashSet};

use provider_protocol::{
    ContentBlock, ConversationItem, FinishReason, PromptCompletion, ProviderError, StreamEvent,
    StreamEventSink, TokenUsage, ToolCall,
};
use serde::Deserialize;

/// `OpenAiSseDecoder` decodes complete `data:` frames from arbitrary byte chunks.
#[derive(Debug, Default)]
pub(crate) struct OpenAiSseDecoder {
    pending: Vec<u8>,
    event_name: Option<String>,
    event_data: Vec<String>,
}

impl OpenAiSseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.pending.extend_from_slice(chunk);
        let mut frames = Vec::new();

        while let Some(newline_index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=newline_index).collect::<Vec<_>>();
            let line = trim_line_end(&line);
            self.apply_line(line, &mut frames)?;
        }

        Ok(frames)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<String>, ProviderError> {
        let mut frames = Vec::new();
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let line = trim_line_end(&pending);
            self.apply_line(line, &mut frames)?;
        }
        self.emit_event_if_complete(&mut frames);
        Ok(frames)
    }

    fn apply_line(&mut self, line: &[u8], frames: &mut Vec<String>) -> Result<(), ProviderError> {
        if line.is_empty() {
            self.emit_event_if_complete(frames);
            return Ok(());
        }

        let line = std::str::from_utf8(line).map_err(|source| {
            ProviderError::Protocol(format!("invalid SSE UTF-8 line: {source}"))
        })?;
        if line.starts_with(':') {
            return Ok(());
        }

        if line == "data" {
            self.event_data.push(String::new());
            return Ok(());
        }
        if line == "event" {
            self.event_name = Some(String::new());
            return Ok(());
        }
        let Some(data) = line.strip_prefix("data:") else {
            if let Some(event_name) = line.strip_prefix("event:") {
                self.event_name = Some(sse_field_value(event_name).to_string());
            }
            return Ok(());
        };
        self.event_data.push(sse_field_value(data).to_string());
        Ok(())
    }

    fn emit_event_if_complete(&mut self, frames: &mut Vec<String>) {
        let event_name = self.event_name.take();
        if self.event_data.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.event_data).join("\n");
        if event_name.as_deref() != Some("keepalive") {
            frames.push(data);
        }
    }
}

fn trim_line_end(mut line: &[u8]) -> &[u8] {
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line = &line[..line.len() - 1];
    }
    line
}

fn sse_field_value(value: &str) -> &str {
    value.strip_prefix(' ').unwrap_or(value)
}

/// `OpenAiStreamState` aggregates chat-completion deltas into core events and response.
#[derive(Debug)]
pub(crate) struct OpenAiStreamState {
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    started_tool_calls: HashSet<usize>,
    finish_reason: Option<FinishReason>,
    usage: Option<TokenUsage>,
    has_started: bool,
}

impl OpenAiStreamState {
    pub(crate) fn new(_model: String) -> Self {
        Self {
            content: String::new(),
            reasoning_content: String::new(),
            tool_calls: BTreeMap::new(),
            started_tool_calls: HashSet::new(),
            finish_reason: None,
            usage: None,
            has_started: false,
        }
    }

    pub(crate) fn apply_data_frame(
        &mut self,
        data: &str,
        sink: &mut (dyn StreamEventSink + Send),
    ) -> Result<(), ProviderError> {
        if !self.has_started {
            self.has_started = true;
            sink.emit(StreamEvent::TurnStarted);
        }

        let chunk = serde_json::from_str::<ChatCompletionChunk>(data).map_err(|source| {
            ProviderError::Protocol(format!("invalid chat completion chunk: {source}"))
        })?;
        if let Some(usage) = chunk.usage {
            let usage = TokenUsage::new(
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
            );
            self.usage = Some(usage);
            sink.emit(StreamEvent::UsageUpdated(usage));
        }

        for choice in chunk.choices {
            if let Some(delta) = choice.delta.content.filter(|delta| !delta.is_empty()) {
                self.content.push_str(&delta);
                sink.emit(StreamEvent::TextDelta(delta));
            }
            if let Some(delta) = choice
                .delta
                .reasoning_content
                .or(choice.delta.reasoning)
                .filter(|delta| !delta.is_empty())
            {
                self.reasoning_content.push_str(&delta);
                sink.emit(StreamEvent::ReasoningDelta(delta));
            }
            for tool_call in choice.delta.tool_calls.unwrap_or_default() {
                self.apply_tool_call_delta(tool_call, sink);
            }
            if let Some(finish_reason) = choice.finish_reason {
                self.finish_reason = Some(finish_reason_from_openai(&finish_reason));
            }
        }

        Ok(())
    }

    pub(crate) fn finish(
        mut self,
        sink: &mut (dyn StreamEventSink + Send),
    ) -> Result<PromptCompletion, ProviderError> {
        if !self.has_started {
            sink.emit(StreamEvent::TurnStarted);
        }

        let tool_calls = self
            .tool_calls
            .iter()
            .map(|(index, call)| call.to_tool_call(*index))
            .collect::<Result<Vec<_>, _>>()?;
        for ((index, _), call) in self.tool_calls.iter().zip(tool_calls.iter()) {
            sink.emit(StreamEvent::ToolCallCompleted {
                index: *index,
                call: call.clone(),
            });
        }

        let finish_reason = self.finish_reason.take().unwrap_or({
            if tool_calls.is_empty() {
                FinishReason::Stop
            } else {
                FinishReason::ToolCalls
            }
        });

        let mut items = Vec::new();

        if !self.reasoning_content.is_empty() {
            items.push(ConversationItem::Reasoning {
                content: self.reasoning_content,
                summary: None,
                encrypted: None,
            });
        }

        let mut assistant_content = Vec::new();
        if !self.content.is_empty() {
            assistant_content.push(ContentBlock::Text(self.content));
        }
        if !assistant_content.is_empty() || !tool_calls.is_empty() {
            items.push(ConversationItem::assistant_with_parts(
                assistant_content,
                tool_calls,
            ));
        }

        let completion = PromptCompletion::new(items, finish_reason, self.usage);
        sink.emit(StreamEvent::TurnCompleted(completion.clone()));
        Ok(completion)
    }

    fn apply_tool_call_delta(
        &mut self,
        delta: OpenAiToolCallDelta,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let index = delta.index;
        let partial = self.tool_calls.entry(index).or_default();
        if let Some(id) = delta.id.filter(|id| !id.is_empty()) {
            partial.call_id = Some(id);
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                partial.name = Some(name);
            }
            if let Some(arguments) = function.arguments.filter(|arguments| !arguments.is_empty()) {
                partial.arguments.push_str(&arguments);
                sink.emit(StreamEvent::ToolCallArgumentsDelta {
                    index,
                    delta: arguments,
                });
            }
        }

        if let (Some(call_id), Some(name)) = (partial.call_id.as_ref(), partial.name.as_ref())
            && self.started_tool_calls.insert(index)
        {
            sink.emit(StreamEvent::ToolCallStarted {
                index,
                call_id: call_id.clone(),
                name: name.clone(),
            });
        }
    }
}

#[derive(Debug, Default)]
struct PartialToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn to_tool_call(&self, index: usize) -> Result<ToolCall, ProviderError> {
        let name = self.name.clone().ok_or_else(|| {
            ProviderError::Protocol(format!("tool call {index} completed without a name"))
        })?;
        let call_id = self.call_id.clone().ok_or_else(|| {
            ProviderError::Protocol(format!("tool call {index} completed without an id"))
        })?;
        let arguments = if self.arguments.trim().is_empty() {
            "{}".to_string()
        } else {
            self.arguments.clone()
        };
        Ok(ToolCall::new(call_id, name, arguments))
    }
}

fn finish_reason_from_openai(value: &str) -> FinishReason {
    match value {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        other => FinishReason::Other(other.to_string()),
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoiceDelta>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceDelta {
    #[serde(default)]
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ConversationItem, FinishReason, StreamEvent, StreamEventSink};

    use super::{OpenAiSseDecoder, OpenAiStreamState};

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
    fn stream_state_aggregates_tool_call_arguments() {
        let mut state = OpenAiStreamState::new("qwen3".to_string());
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
    fn stream_state_waits_for_tool_call_id_before_started_event() {
        let mut state = OpenAiStreamState::new("gpt-5-mini".to_string());
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
        let mut state = OpenAiStreamState::new("gpt-5-mini".to_string());
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
        let mut state = OpenAiStreamState::new("qwen3".to_string());
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
    fn stream_state_does_not_emit_empty_assistant_item_for_reasoning_only() {
        let mut state = OpenAiStreamState::new("qwen3".to_string());
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
}
