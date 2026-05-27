use provider_protocol::{
    Message, PromptRequest, PromptResponse, ProviderClient, ProviderError, StreamEvent,
    StreamEventSink,
};
use tokio_util::sync::CancellationToken;

use crate::error::ToolLoopError;

use super::{ToolLoopClock, ToolLoopProgress, state::RuntimeTurnState};

pub(super) async fn stream_provider_turn<C, F>(
    client: &C,
    request: PromptRequest,
    cancellation: &CancellationToken,
    clock: &ToolLoopClock,
    state: &mut RuntimeTurnState,
    on_progress: &mut F,
) -> Result<PromptResponse, ToolLoopError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(ToolLoopProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(ToolLoopError::Cancelled);
    }
    on_progress(ToolLoopProgress::ProviderTurnStarted);
    state.start_provider_turn(clock.now());
    let mut provider_response = None;
    let result = {
        let mut sink = RuntimeStreamSink {
            state,
            on_progress,
            clock,
            provider_response: &mut provider_response,
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(ToolLoopError::Cancelled),
            result = client.stream_prompt(request, &mut sink) => result,
        }
    };

    match result {
        Ok(response) => {
            let response = provider_response.unwrap_or(response);
            let completed_at = clock.now();
            state.observe_response_tool_calls_completed(&response, completed_at, on_progress);
            state.complete_provider_turn(&response, completed_at);
            Ok(response)
        }
        Err(ProviderError::Transport(message)) if cancellation.is_cancelled() => {
            let _ = message;
            Err(ToolLoopError::Cancelled)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn append_provider_context_message<F>(
    message: Message,
    appended_messages: &mut Vec<Message>,
    on_progress: &mut F,
) where
    F: FnMut(ToolLoopProgress),
{
    on_progress(ToolLoopProgress::ProviderContextMessage {
        message: message.clone(),
    });
    appended_messages.push(message);
}

struct RuntimeStreamSink<'a, F>
where
    F: FnMut(ToolLoopProgress),
{
    state: &'a mut RuntimeTurnState,
    on_progress: &'a mut F,
    clock: &'a ToolLoopClock,
    provider_response: &'a mut Option<PromptResponse>,
}

impl<F> StreamEventSink for RuntimeStreamSink<'_, F>
where
    F: FnMut(ToolLoopProgress),
{
    fn emit(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::MessageStarted => self.state.mark_request_started(self.clock.now()),
            StreamEvent::TextDelta(content) => {
                self.state
                    .observe_content_chunk(&content, self.clock.now(), self.on_progress)
            }
            StreamEvent::ReasoningDelta(content) => {
                self.state
                    .observe_reasoning_chunk(&content, self.clock.now(), self.on_progress)
            }
            StreamEvent::UsageUpdated(usage) => {
                if let Some(output_tokens) = usage.output_tokens {
                    self.state
                        .record_provider_output_usage(output_tokens as usize);
                }
            }
            StreamEvent::ToolCallStarted { index, call_id, .. } => {
                self.state.observe_tool_call_started(index, call_id);
            }
            StreamEvent::ToolCallArgumentsDelta { index, delta } => {
                self.state.observe_tool_call_arguments_delta(
                    index,
                    &delta,
                    self.clock.now(),
                    self.on_progress,
                );
            }
            StreamEvent::ToolCallCompleted { index, call } => {
                self.state.observe_tool_call_completed(
                    index,
                    &call,
                    self.clock.now(),
                    self.on_progress,
                );
            }
            StreamEvent::MessageCompleted(response) => {
                *self.provider_response = Some(response);
            }
        }
    }
}
