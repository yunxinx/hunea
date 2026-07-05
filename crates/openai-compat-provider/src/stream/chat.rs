use std::collections::{BTreeMap, HashSet};

use provider_protocol::{
    ContentBlock, ConversationItem, FinishReason, PromptCompletion, ProviderError, StreamEvent,
    StreamEventSink, TokenUsage,
};

use super::{ChatCompletionChunk, OpenAiToolCallDelta, PartialToolCall, finish_reason_from_openai};

/// `OpenAiStreamState` 将 chat-completion delta 聚合为 core stream events 与最终响应。
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
