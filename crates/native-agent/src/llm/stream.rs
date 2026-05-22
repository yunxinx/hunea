use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt as _;
use mo_core::session::NativeLlmRequest;
use mo_core::token_count::StreamingTokenProgress;
use mo_tools::{
    SharedToolPermissionHandler, ToolExecutorRegistry, ToolRegistry, rig::RigToolServer,
};
use rig_core::{
    agent::{AgentBuilder, MultiTurnStreamItem},
    completion::{CompletionModel, GetTokenUsage},
    message::{Message as RigMessage, ToolFunction},
    streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat},
};
use tokio_util::sync::CancellationToken;

use crate::{
    NativeAgentError, NativeAgentRequest,
    agent::{NativeAgentCompletion, NativeAgentProgress},
    llm::{
        NativeLlmError, rig_message_from_chat_message,
        tool_errors::NativeAgentToolErrorFormatter,
        tools::{
            runtime_tool_activity_from_rig_call, runtime_tool_activity_update_from_rig_result,
            tool_result_text,
        },
    },
};

pub use mo_core::session::NativeLlmPerformanceMetrics;

/// `NativeLlmProgress` 描述原生 runtime 流式输出期间可用于 UI 的进度事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLlmProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
}

// Rig 的 `multi_turn` 以 `usize` 表示上限；这里用最大安全值表达 Lumos 默认不设上限，
// 同时避开 Rig 内部 `max_turns + 1` 的溢出风险。
const RIG_UNBOUNDED_TOOL_TURNS: usize = usize::MAX - 1;

pub(super) async fn run_rig_agent<M, F>(
    model: M,
    request: &NativeAgentRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    tool_max_turns: Option<usize>,
    permission_handler: Option<SharedToolPermissionHandler>,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage + Send + 'static,
    F: FnMut(NativeAgentProgress),
{
    let (prompt, history) = prompt_and_history_for_request(request.llm_request())?;
    let error_formatter = Arc::new(NativeAgentToolErrorFormatter);
    let tool_server = match permission_handler {
        Some(permission_handler) => {
            RigToolServer::from_executor_with_permission_handler(
                executor,
                cancellation.clone(),
                error_formatter,
                permission_handler,
            )
            .await
        }
        None => {
            RigToolServer::from_executor_with_error_formatter(
                executor,
                cancellation.clone(),
                error_formatter,
            )
            .await
        }
    }
    .map_err(|error| NativeLlmError::Provider(error.to_string()))?;
    let tool_definitions = tool_server.definitions();
    let agent = AgentBuilder::new(model)
        .tool_server_handle(tool_server.handle().clone())
        .build();
    let tool_calls = Arc::new(StreamedToolCallIndex::default());
    let mut accumulator = RigAgentAccumulator::new(request.llm_request().model_id.clone());
    let request_started_at = Instant::now();
    let max_turns = rig_tool_turn_limit(tool_max_turns);

    let mut stream = tokio::select! {
        _ = cancellation.cancelled() => return Err(NativeAgentError::Cancelled),
        stream = agent
            .stream_chat(prompt, history)
            .multi_turn(max_turns) => stream,
    };

    accumulator.mark_request_started(request_started_at);

    loop {
        let event = tokio::select! {
            _ = cancellation.cancelled() => return Err(NativeAgentError::Cancelled),
            event = stream.next() => event,
        };
        let Some(event) = event else {
            break;
        };

        match event.map_err(rig_streaming_error)? {
            MultiTurnStreamItem::StreamAssistantItem(content) => {
                handle_streamed_assistant_content(
                    content,
                    &tool_definitions,
                    Arc::clone(&tool_calls),
                    &mut accumulator,
                    on_progress,
                );
            }
            MultiTurnStreamItem::StreamUserItem(content) => {
                handle_streamed_user_content(
                    content,
                    &tool_definitions,
                    &tool_server,
                    Arc::clone(&tool_calls),
                    on_progress,
                );
            }
            MultiTurnStreamItem::FinalResponse(final_response) => {
                accumulator.capture_final_response(
                    final_response.response(),
                    final_response.usage().output_tokens as usize,
                );
            }
            _ => {}
        }
    }

    if let Some(total_tokens) = accumulator.progress.flush(Instant::now()) {
        on_progress(NativeAgentProgress::OutputTokens { total_tokens });
    }

    Ok(accumulator.finish_at(Instant::now()))
}

fn handle_streamed_assistant_content<F>(
    content: StreamedAssistantContent<impl Clone + Unpin + GetTokenUsage>,
    tool_definitions: &ToolRegistry,
    tool_calls: Arc<StreamedToolCallIndex>,
    accumulator: &mut RigAgentAccumulator,
    on_progress: &mut F,
) where
    F: FnMut(NativeAgentProgress),
{
    match content {
        StreamedAssistantContent::Text(text) => {
            accumulator.observe_content_chunk(text.text(), Instant::now(), on_progress);
        }
        StreamedAssistantContent::ToolCall {
            tool_call,
            internal_call_id,
        } => {
            let runtime_activity = runtime_tool_activity_from_rig_call(
                &tool_call,
                &internal_call_id,
                tool_definitions,
            );
            tool_calls.insert(internal_call_id.clone(), tool_call);
            on_progress(NativeAgentProgress::ToolActivityStarted {
                activity: runtime_activity,
            });
        }
        StreamedAssistantContent::ToolCallDelta { .. } => {}
        StreamedAssistantContent::Reasoning(reasoning) => {
            accumulator.observe_reasoning_chunk(
                &reasoning.display_text(),
                Instant::now(),
                on_progress,
            );
        }
        StreamedAssistantContent::ReasoningDelta { reasoning, .. } => {
            accumulator.observe_reasoning_chunk(&reasoning, Instant::now(), on_progress);
        }
        StreamedAssistantContent::Final(final_response) => {
            if let Some(usage) = final_response.token_usage() {
                accumulator.final_output_tokens = Some(usage.output_tokens as usize);
            }
        }
    }
}

fn handle_streamed_user_content<F>(
    content: StreamedUserContent,
    tool_definitions: &ToolRegistry,
    tool_server: &RigToolServer,
    tool_calls: Arc<StreamedToolCallIndex>,
    on_progress: &mut F,
) where
    F: FnMut(NativeAgentProgress),
{
    let StreamedUserContent::ToolResult {
        tool_result,
        internal_call_id,
    } = content;
    let tool_call = tool_calls
        .remove(&internal_call_id)
        .unwrap_or_else(|| fallback_tool_call_from_result(&tool_result));
    let result_text = tool_result_text(&tool_result.content);
    let result_details = tool_server.take_tool_result_details(
        &tool_call.function.name,
        &tool_call.function.arguments,
        &result_text,
    );
    let update = runtime_tool_activity_update_from_rig_result(
        &internal_call_id,
        &tool_call,
        &result_text,
        result_details,
        tool_definitions,
    );

    on_progress(NativeAgentProgress::ToolActivityUpdated { update });
}

fn prompt_and_history_for_request(
    request: &NativeLlmRequest,
) -> Result<(RigMessage, Vec<RigMessage>), NativeLlmError> {
    let mut messages = request
        .messages
        .clone()
        .into_iter()
        .map(rig_message_from_chat_message)
        .collect::<Vec<_>>();
    let prompt = messages.pop().ok_or_else(|| NativeLlmError::EmptyPrompt {
        provider_id: request.provider_id.clone(),
    })?;
    Ok((prompt, messages))
}

fn rig_tool_turn_limit(tool_max_turns: Option<usize>) -> usize {
    tool_max_turns.unwrap_or(RIG_UNBOUNDED_TOOL_TURNS)
}

#[derive(Default)]
struct StreamedToolCallIndex {
    streamed_calls: Mutex<HashMap<String, rig_core::message::ToolCall>>,
}

impl StreamedToolCallIndex {
    fn insert(&self, internal_call_id: String, call: rig_core::message::ToolCall) {
        self.streamed_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .insert(internal_call_id, call);
    }

    fn remove(&self, internal_call_id: &str) -> Option<rig_core::message::ToolCall> {
        self.streamed_calls
            .lock()
            .expect("rig tool state lock should not be poisoned")
            .remove(internal_call_id)
    }
}

fn fallback_tool_call_from_result(
    tool_result: &rig_core::message::ToolResult,
) -> rig_core::message::ToolCall {
    rig_core::message::ToolCall::new(
        tool_result.id.clone(),
        ToolFunction::new("tool".to_string(), serde_json::json!({})),
    )
    .with_call_id(
        tool_result
            .call_id
            .clone()
            .unwrap_or_else(|| tool_result.id.clone()),
    )
}

struct RigAgentAccumulator {
    content: String,
    final_content: Option<String>,
    reasoning_content: String,
    progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
    request_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    final_output_tokens: Option<usize>,
}

impl RigAgentAccumulator {
    fn new(model_id: String) -> Self {
        Self {
            content: String::new(),
            final_content: None,
            reasoning_content: String::new(),
            progress: StreamingTokenProgress::new(model_id),
            is_thinking: false,
            reasoning_started_at: None,
            reasoning_finished_at: None,
            request_started_at: None,
            first_token_at: None,
            final_output_tokens: None,
        }
    }

    fn mark_request_started(&mut self, now: Instant) {
        self.request_started_at = Some(now);
    }

    fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeAgentProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(NativeAgentProgress::Thinking { is_thinking: false });
        }
        self.content.push_str(content);
        on_progress(NativeAgentProgress::AssistantDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeAgentProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(NativeAgentProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        on_progress(NativeAgentProgress::ReasoningDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_token_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeAgentProgress),
    ) {
        if let Some(total_tokens) = self.progress.observe_delta(content, now) {
            on_progress(NativeAgentProgress::OutputTokens { total_tokens });
        }
    }

    fn capture_final_response(&mut self, content: &str, output_tokens: usize) {
        self.final_content = Some(content.to_string());
        self.final_output_tokens = Some(output_tokens);
    }

    fn finish_at(self, finished_at: Instant) -> NativeAgentCompletion {
        let metrics = self.performance_metrics(finished_at);
        let reasoning_content = trim_outer_blank_lines(&self.reasoning_content);
        let reasoning_duration = self.reasoning_duration();
        let content = self
            .final_content
            .unwrap_or(self.content)
            .trim_end()
            .to_string();
        NativeAgentCompletion {
            response: mo_core::session::NativeAgentResponse {
                content,
                reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
                reasoning_duration,
            },
            metrics,
        }
    }

    fn performance_metrics(&self, finished_at: Instant) -> Option<NativeLlmPerformanceMetrics> {
        let request_started_at = self.request_started_at?;
        let first_token_at = self.first_token_at?;
        Some(NativeLlmPerformanceMetrics {
            latency: first_token_at.saturating_duration_since(request_started_at),
            output_tokens: self
                .final_output_tokens
                .unwrap_or_else(|| self.progress.total_tokens()),
            duration: finished_at.saturating_duration_since(request_started_at),
        })
    }

    fn reasoning_duration(&self) -> Option<Duration> {
        if self.reasoning_content.trim().is_empty() {
            return None;
        }

        let started_at = self.reasoning_started_at?;
        let finished_at = self.reasoning_finished_at.unwrap_or(started_at);
        Some(finished_at.saturating_duration_since(started_at))
    }
}

fn trim_outer_blank_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("start exists when at least one non-blank line exists");

    lines[start..=end].join("\n")
}

fn rig_streaming_error(source: rig_core::agent::StreamingError) -> NativeAgentError {
    NativeLlmError::Provider(source.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::{RIG_UNBOUNDED_TOOL_TURNS, rig_tool_turn_limit};

    #[test]
    fn rig_tool_turn_limit_defaults_to_effectively_unbounded() {
        assert_eq!(rig_tool_turn_limit(None), RIG_UNBOUNDED_TOOL_TURNS);
    }

    #[test]
    fn rig_tool_turn_limit_uses_configured_limit() {
        assert_eq!(rig_tool_turn_limit(Some(11)), 11);
    }
}
