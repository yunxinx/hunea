use std::collections::{BTreeMap, HashSet};

use provider_protocol::{ProviderError, StreamEvent, StreamEventSink, ToolCall};

use super::{OpenAiToolCallDelta, ResponsesOutputItem};

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

#[derive(Debug, Default)]
pub(super) struct ToolCallAccumulator {
    partials: BTreeMap<usize, PartialToolCall>,
    started: HashSet<usize>,
}

impl ToolCallAccumulator {
    pub(super) fn is_empty(&self) -> bool {
        self.partials.is_empty()
    }

    pub(super) fn apply_chat_delta(
        &mut self,
        delta: OpenAiToolCallDelta,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let index = delta.index;
        let partial = self.partials.entry(index).or_default();
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
        self.emit_started_if_ready(index, sink);
    }

    pub(super) fn apply_responses_item(
        &mut self,
        index: usize,
        item: ResponsesOutputItem,
        arguments_mode: ResponsesToolArgumentsMode,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let partial = self.partials.entry(index).or_default();
        if let Some(call_id) = item.call_id.filter(|value| !value.is_empty()) {
            partial.call_id = Some(call_id);
        }
        if let Some(name) = item.name.filter(|value| !value.is_empty()) {
            partial.name = Some(name);
        }
        if let Some(arguments) = item.arguments.filter(|value| !value.is_empty()) {
            match arguments_mode {
                ResponsesToolArgumentsMode::Append => partial.arguments.push_str(&arguments),
                ResponsesToolArgumentsMode::Replace => partial.arguments = arguments,
            }
        }
        self.emit_started_if_ready(index, sink);
    }

    pub(super) fn append_arguments_delta(
        &mut self,
        index: usize,
        delta: String,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        self.partials
            .entry(index)
            .or_default()
            .arguments
            .push_str(&delta);
        sink.emit(StreamEvent::ToolCallArgumentsDelta { index, delta });
    }

    pub(super) fn replace_arguments_emitting_missing_delta(
        &mut self,
        index: usize,
        arguments: String,
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        let partial = self.partials.entry(index).or_default();
        if arguments.starts_with(&partial.arguments) {
            let delta = arguments[partial.arguments.len()..].to_string();
            if !delta.is_empty() {
                sink.emit(StreamEvent::ToolCallArgumentsDelta { index, delta });
            }
        }
        partial.arguments = arguments;
    }

    pub(super) fn finalized_calls(&self) -> Result<Vec<(usize, ToolCall)>, ProviderError> {
        self.partials
            .iter()
            .map(|(index, call)| Ok((*index, call.to_tool_call(*index)?)))
            .collect()
    }

    pub(super) fn emit_completed(
        &self,
        calls: &[(usize, ToolCall)],
        sink: &mut (dyn StreamEventSink + Send),
    ) {
        for (index, call) in calls {
            sink.emit(StreamEvent::ToolCallCompleted {
                index: *index,
                call: call.clone(),
            });
        }
    }

    fn emit_started_if_ready(&mut self, index: usize, sink: &mut (dyn StreamEventSink + Send)) {
        let Some(partial) = self.partials.get(&index) else {
            return;
        };
        if let (Some(call_id), Some(name)) = (partial.call_id.as_ref(), partial.name.as_ref())
            && self.started.insert(index)
        {
            sink.emit(StreamEvent::ToolCallStarted {
                index,
                call_id: call_id.clone(),
                name: name.clone(),
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponsesToolArgumentsMode {
    Append,
    Replace,
}
