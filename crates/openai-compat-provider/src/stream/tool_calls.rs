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
    ) -> Result<(), ProviderError> {
        let index = delta.index;
        let has_started = self.started.contains(&index);
        let partial = self.partials.entry(index).or_default();
        if let Some(id) = delta.id.filter(|id| !id.is_empty()) {
            apply_call_id(index, partial, has_started, id)?;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                apply_name(index, partial, has_started, name)?;
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
        Ok(())
    }

    pub(super) fn apply_responses_item(
        &mut self,
        index: usize,
        item: ResponsesOutputItem,
        arguments_mode: ResponsesToolArgumentsMode,
        sink: &mut (dyn StreamEventSink + Send),
    ) -> Result<(), ProviderError> {
        let has_started = self.started.contains(&index);
        let partial = self.partials.entry(index).or_default();
        if let Some(call_id) = item.call_id.filter(|value| !value.is_empty()) {
            apply_call_id(index, partial, has_started, call_id)?;
        }
        if let Some(name) = item.name.filter(|value| !value.is_empty()) {
            apply_name(index, partial, has_started, name)?;
        }
        if let Some(arguments) = item.arguments.filter(|value| !value.is_empty()) {
            match arguments_mode {
                ResponsesToolArgumentsMode::Append => {
                    partial.arguments.push_str(&arguments);
                    sink.emit(StreamEvent::ToolCallArgumentsDelta {
                        index,
                        delta: arguments,
                    });
                }
                ResponsesToolArgumentsMode::Replace => {
                    replace_arguments(index, partial, arguments, sink)?;
                }
            }
        }
        self.emit_started_if_ready(index, sink);
        Ok(())
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
        arguments: Option<String>,
        sink: &mut (dyn StreamEventSink + Send),
    ) -> Result<(), ProviderError> {
        let Some(arguments) = arguments.filter(|value| !value.is_empty()) else {
            return Ok(());
        };
        let partial = self.partials.entry(index).or_default();
        replace_arguments(index, partial, arguments, sink)
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

fn apply_call_id(
    index: usize,
    partial: &mut PartialToolCall,
    has_started: bool,
    call_id: String,
) -> Result<(), ProviderError> {
    if has_started
        && partial
            .call_id
            .as_ref()
            .is_some_and(|current| current != &call_id)
    {
        return Err(ProviderError::Protocol(format!(
            "tool call {index} id changed after start"
        )));
    }
    partial.call_id = Some(call_id);
    Ok(())
}

fn apply_name(
    index: usize,
    partial: &mut PartialToolCall,
    has_started: bool,
    name: String,
) -> Result<(), ProviderError> {
    if has_started
        && partial
            .name
            .as_ref()
            .is_some_and(|current| current != &name)
    {
        return Err(ProviderError::Protocol(format!(
            "tool call {index} name changed after start"
        )));
    }
    partial.name = Some(name);
    Ok(())
}

fn replace_arguments(
    index: usize,
    partial: &mut PartialToolCall,
    arguments: String,
    sink: &mut (dyn StreamEventSink + Send),
) -> Result<(), ProviderError> {
    let Some(delta) = arguments.strip_prefix(partial.arguments.as_str()) else {
        return Err(ProviderError::Protocol(format!(
            "final arguments for tool call {index} do not extend streamed arguments"
        )));
    };
    if !delta.is_empty() {
        sink.emit(StreamEvent::ToolCallArgumentsDelta {
            index,
            delta: delta.to_string(),
        });
    }
    partial.arguments = arguments;
    Ok(())
}
