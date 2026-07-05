use std::collections::BTreeMap;

use provider_protocol::{
    ContentBlock, ConversationItem, FinishReason, PromptCompletion, ProviderError, StreamEvent,
    StreamEventSink, TokenUsage,
};

use super::{
    ResponsesOutputItem, ResponsesStreamEvent, ResponsesToolArgumentsMode, ToolCallAccumulator,
    finish_reason_from_responses_status,
};

/// `OpenAiResponsesStreamState` 将 Responses API events 聚合为 core stream events 与最终响应。
#[derive(Debug, Default)]
pub(crate) struct OpenAiResponsesStreamState {
    text_outputs: BTreeMap<usize, String>,
    reasoning_outputs: BTreeMap<usize, String>,
    tool_calls: ToolCallAccumulator,
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
                self.tool_calls
                    .append_arguments_delta(output_index, delta, sink);
            }
            ResponsesStreamEvent::FunctionCallArgumentsDone {
                output_index,
                arguments,
            } => {
                self.tool_calls.replace_arguments_emitting_missing_delta(
                    output_index,
                    arguments,
                    sink,
                );
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
        let indexed_tool_calls = if terminal_finish_reason == FinishReason::Stop {
            self.tool_calls.finalized_calls()?
        } else {
            Vec::new()
        };
        self.tool_calls.emit_completed(&indexed_tool_calls, sink);
        let tool_calls = indexed_tool_calls
            .into_iter()
            .map(|(_, call)| call)
            .collect::<Vec<_>>();

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
        self.tool_calls.apply_responses_item(
            output_index,
            item,
            ResponsesToolArgumentsMode::Append,
            sink,
        );
    }

    fn apply_output_item_done(
        &mut self,
        output_index: usize,
        item: ResponsesOutputItem,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        match item.kind.as_deref() {
            Some("function_call") => {
                self.tool_calls.apply_responses_item(
                    output_index,
                    item,
                    ResponsesToolArgumentsMode::Replace,
                    sink,
                );
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
}
