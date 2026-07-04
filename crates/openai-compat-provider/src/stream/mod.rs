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

/// `OpenAiResponsesStreamState` aggregates Responses API events into core events and response.
#[derive(Debug, Default)]
pub(crate) struct OpenAiResponsesStreamState {
    text_outputs: BTreeMap<usize, String>,
    reasoning_outputs: BTreeMap<usize, String>,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    started_tool_calls: HashSet<usize>,
    finish_reason: Option<FinishReason>,
    usage: Option<TokenUsage>,
    has_started: bool,
    saw_terminal_event: bool,
}

impl OpenAiResponsesStreamState {
    pub(crate) fn apply_data_frame(
        &mut self,
        data: &str,
        sink: &mut (dyn StreamEventSink + Send),
    ) -> Result<(), ProviderError> {
        if !self.has_started {
            self.has_started = true;
            sink.emit(StreamEvent::TurnStarted);
        }

        let event = serde_json::from_str::<ResponsesStreamEvent>(data).map_err(|source| {
            ProviderError::Protocol(format!("invalid responses stream event: {source}"))
        })?;

        match event {
            ResponsesStreamEvent::OutputItemAdded { output_index, item } => {
                self.apply_output_item_added(output_index, item, sink);
            }
            ResponsesStreamEvent::OutputTextDelta {
                output_index,
                delta,
            }
            | ResponsesStreamEvent::RefusalDelta {
                output_index,
                delta,
            } => {
                if !delta.is_empty() {
                    self.text_outputs
                        .entry(output_index)
                        .or_default()
                        .push_str(&delta);
                    sink.emit(StreamEvent::TextDelta(delta));
                }
            }
            ResponsesStreamEvent::ReasoningTextDelta {
                output_index,
                delta,
            }
            | ResponsesStreamEvent::ReasoningSummaryTextDelta {
                output_index,
                delta,
            } => {
                if !delta.is_empty() {
                    self.reasoning_outputs
                        .entry(output_index)
                        .or_default()
                        .push_str(&delta);
                    sink.emit(StreamEvent::ReasoningDelta(delta));
                }
            }
            ResponsesStreamEvent::ReasoningSummaryPartDone { output_index } => {
                self.reasoning_outputs
                    .entry(output_index)
                    .or_default()
                    .push_str("\n\n");
                sink.emit(StreamEvent::ReasoningDelta("\n\n".to_string()));
            }
            ResponsesStreamEvent::FunctionCallArgumentsDelta {
                output_index,
                delta,
            } => {
                let partial = self.tool_calls.entry(output_index).or_default();
                partial.arguments.push_str(&delta);
                sink.emit(StreamEvent::ToolCallArgumentsDelta {
                    index: output_index,
                    delta,
                });
            }
            ResponsesStreamEvent::FunctionCallArgumentsDone {
                output_index,
                arguments,
            } => {
                let partial = self.tool_calls.entry(output_index).or_default();
                if arguments.starts_with(&partial.arguments) {
                    let delta = arguments[partial.arguments.len()..].to_string();
                    if !delta.is_empty() {
                        sink.emit(StreamEvent::ToolCallArgumentsDelta {
                            index: output_index,
                            delta,
                        });
                    }
                }
                partial.arguments = arguments;
            }
            ResponsesStreamEvent::OutputItemDone { output_index, item } => {
                self.apply_output_item_done(output_index, item, sink);
            }
            ResponsesStreamEvent::Completed { response }
            | ResponsesStreamEvent::Incomplete { response } => {
                self.saw_terminal_event = true;
                self.finish_reason = Some(finish_reason_from_responses_status(
                    response.status.as_deref(),
                ));
                if let Some(usage) = response.usage {
                    let usage = TokenUsage::new(
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.total_tokens,
                    );
                    self.usage = Some(usage);
                    sink.emit(StreamEvent::UsageUpdated(usage));
                }
            }
            ResponsesStreamEvent::Failed { response } => {
                self.saw_terminal_event = true;
                let message = response
                    .error
                    .and_then(|error| error.message)
                    .or(response
                        .incomplete_details
                        .and_then(|details| details.reason))
                    .unwrap_or_else(|| "responses stream failed".to_string());
                return Err(ProviderError::Provider {
                    status: None,
                    message,
                });
            }
            ResponsesStreamEvent::Error { code, message } => {
                return Err(ProviderError::Provider {
                    status: None,
                    message: format!("{code}: {message}"),
                });
            }
            ResponsesStreamEvent::Other => {}
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
        if !self.saw_terminal_event {
            return Err(ProviderError::Protocol(
                "Responses stream ended before a terminal response event".to_string(),
            ));
        }

        let terminal_finish_reason = self.finish_reason.take().unwrap_or(FinishReason::Stop);
        let tool_calls = if terminal_finish_reason == FinishReason::Stop {
            self.tool_calls
                .iter()
                .map(|(index, call)| call.to_tool_call(*index))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        for ((index, _), call) in self.tool_calls.iter().zip(tool_calls.iter()) {
            sink.emit(StreamEvent::ToolCallCompleted {
                index: *index,
                call: call.clone(),
            });
        }

        let mut items = Vec::new();
        let reasoning_content = self
            .reasoning_outputs
            .values()
            .map(String::as_str)
            .collect::<String>();
        if !reasoning_content.trim().is_empty() {
            items.push(ConversationItem::Reasoning {
                content: reasoning_content.trim_end().to_string(),
                summary: None,
                encrypted: None,
            });
        }
        let mut assistant_content = Vec::new();
        let content = self
            .text_outputs
            .values()
            .map(String::as_str)
            .collect::<String>();
        if !content.is_empty() {
            assistant_content.push(ContentBlock::Text(content));
        }
        if !assistant_content.is_empty() || !tool_calls.is_empty() {
            items.push(ConversationItem::assistant_with_parts(
                assistant_content,
                tool_calls,
            ));
        }

        let has_tool_calls = items.iter().any(|item| item.tool_calls().next().is_some());
        let finish_reason = if has_tool_calls && terminal_finish_reason == FinishReason::Stop {
            FinishReason::ToolCalls
        } else {
            terminal_finish_reason
        };
        let completion = PromptCompletion::new(items, finish_reason, self.usage);
        sink.emit(StreamEvent::TurnCompleted(completion.clone()));
        Ok(completion)
    }

    fn apply_output_item_added(
        &mut self,
        output_index: usize,
        item: ResponsesOutputItem,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        if item.kind.as_deref() != Some("function_call") {
            return;
        }
        let partial = self.tool_calls.entry(output_index).or_default();
        if let Some(call_id) = item.call_id.filter(|value| !value.is_empty()) {
            partial.call_id = Some(call_id);
        }
        if let Some(name) = item.name.filter(|value| !value.is_empty()) {
            partial.name = Some(name);
        }
        if let Some(arguments) = item.arguments.filter(|value| !value.is_empty()) {
            partial.arguments.push_str(&arguments);
        }
        self.emit_responses_tool_call_started(output_index, sink);
    }

    fn apply_output_item_done(
        &mut self,
        output_index: usize,
        item: ResponsesOutputItem,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        match item.kind.as_deref() {
            Some("function_call") => {
                let partial = self.tool_calls.entry(output_index).or_default();
                if let Some(call_id) = item.call_id.filter(|value| !value.is_empty()) {
                    partial.call_id = Some(call_id);
                }
                if let Some(name) = item.name.filter(|value| !value.is_empty()) {
                    partial.name = Some(name);
                }
                if let Some(arguments) = item.arguments {
                    partial.arguments = arguments;
                }
                self.emit_responses_tool_call_started(output_index, sink);
            }
            Some("message") => {
                if let Some(text) = item.visible_output_text().filter(|value| !value.is_empty()) {
                    self.apply_final_text_output(output_index, text, sink);
                }
            }
            Some("reasoning") => {
                if let Some(text) = item
                    .visible_reasoning_text()
                    .filter(|value| !value.is_empty())
                {
                    self.apply_final_reasoning_output(output_index, text, sink);
                }
            }
            _ => {}
        }
    }

    fn apply_final_text_output(
        &mut self,
        output_index: usize,
        final_text: String,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let current = self.text_outputs.entry(output_index).or_default();
        if let Some(delta) = final_text.strip_prefix(current.as_str())
            && !delta.is_empty()
        {
            sink.emit(StreamEvent::TextDelta(delta.to_string()));
        }
        *current = final_text;
    }

    fn apply_final_reasoning_output(
        &mut self,
        output_index: usize,
        final_text: String,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let current = self.reasoning_outputs.entry(output_index).or_default();
        if let Some(delta) = final_text.strip_prefix(current.as_str())
            && !delta.is_empty()
        {
            sink.emit(StreamEvent::ReasoningDelta(delta.to_string()));
        }
        *current = final_text;
    }

    fn emit_responses_tool_call_started(
        &mut self,
        output_index: usize,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let Some(partial) = self.tool_calls.get(&output_index) else {
            return;
        };
        if let (Some(call_id), Some(name)) = (partial.call_id.as_ref(), partial.name.as_ref())
            && self.started_tool_calls.insert(output_index)
        {
            sink.emit(StreamEvent::ToolCallStarted {
                index: output_index,
                call_id: call_id.clone(),
                name: name.clone(),
            });
        }
    }
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

        let terminal_finish_reason = self.finish_reason.take();
        let has_streamed_tool_calls = !self.tool_calls.is_empty();
        let should_finalize_tool_calls = match terminal_finish_reason.as_ref() {
            Some(FinishReason::ToolCalls) => true,
            Some(FinishReason::Stop) | None => has_streamed_tool_calls,
            Some(_) => false,
        };
        let tool_calls = if should_finalize_tool_calls {
            self.tool_calls
                .iter()
                .map(|(index, call)| call.to_tool_call(*index))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        for ((index, _), call) in self.tool_calls.iter().zip(tool_calls.iter()) {
            sink.emit(StreamEvent::ToolCallCompleted {
                index: *index,
                call: call.clone(),
            });
        }

        let finish_reason = match terminal_finish_reason {
            Some(FinishReason::Stop) | None if !tool_calls.is_empty() => FinishReason::ToolCalls,
            Some(reason) => reason,
            None => FinishReason::Stop,
        };

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

fn finish_reason_from_responses_status(status: Option<&str>) -> FinishReason {
    match status {
        Some("completed") => FinishReason::Stop,
        Some("incomplete") => FinishReason::Length,
        Some("failed" | "cancelled") => FinishReason::Other(status.unwrap().to_string()),
        Some(other) => FinishReason::Other(other.to_string()),
        None => FinishReason::Stop,
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

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponsesStreamEvent {
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: ResponsesOutputItem,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        item: ResponsesOutputItem,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.refusal.delta")]
    RefusalDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        #[serde(default)]
        output_index: usize,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        output_index: usize,
        #[serde(default)]
        arguments: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: ResponsesTerminalResponse },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponsesTerminalResponse },
    #[serde(rename = "response.failed")]
    Failed { response: ResponsesTerminalResponse },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesOutputItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
    content: Option<Vec<ResponsesOutputContent>>,
    summary: Option<Vec<ResponsesOutputContent>>,
}

impl ResponsesOutputItem {
    fn visible_output_text(&self) -> Option<String> {
        let content = self.content.as_ref()?;
        let text = content
            .iter()
            .filter(|item| matches!(item.kind.as_deref(), Some("output_text" | "refusal")))
            .filter_map(|item| item.text.as_deref())
            .collect::<String>();
        (!text.is_empty()).then_some(text)
    }

    fn visible_reasoning_text(&self) -> Option<String> {
        let blocks = self.summary.as_ref().or(self.content.as_ref())?;
        let text = blocks
            .iter()
            .filter_map(|item| item.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesOutputContent {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesTerminalResponse {
    status: Option<String>,
    usage: Option<ResponsesUsage>,
    error: Option<ResponsesError>,
    incomplete_details: Option<ResponsesIncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ResponsesError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesIncompleteDetails {
    reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use provider_protocol::{ConversationItem, FinishReason, StreamEvent, StreamEventSink};

    use super::{OpenAiResponsesStreamState, OpenAiSseDecoder, OpenAiStreamState};

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
    fn stream_state_omits_incomplete_tool_call_when_finish_reason_is_length() {
        let mut state = OpenAiStreamState::new("qwen3".to_string());
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
