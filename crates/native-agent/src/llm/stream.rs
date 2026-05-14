use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt as _;
use mo_core::{
    session::NativeLlmRequest,
    token_count::StreamingTokenProgress,
    tools::{RuntimeToolCall, RuntimeToolExecutor, RuntimeToolResult},
};
use rig_core::{
    agent::{AgentBuilder, MultiTurnStreamItem},
    completion::{CompletionModel, GetTokenUsage},
    message::Message as RigMessage,
    streaming::{StreamedAssistantContent, StreamedUserContent, StreamingChat},
};
use tokio_util::sync::CancellationToken;

use crate::{
    NativeAgentError, NativeAgentRequest,
    agent::{NativeAgentCompletion, NativeAgentProgress},
    llm::{
        NativeLlmError, rig_message_from_chat_message,
        tools::{
            RigToolExecutionState, RigToolProgressHook, build_rig_tools_for_request,
            runtime_tool_call_from_rig, tool_result_text,
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

const MAX_AGENT_TOOL_ROUNDS: usize = 8;

pub(super) async fn run_rig_agent<M, F>(
    model: M,
    request: &NativeAgentRequest,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: &CancellationToken,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    M: CompletionModel + 'static,
    M::StreamingResponse: GetTokenUsage + Send + 'static,
    F: FnMut(NativeAgentProgress),
{
    let (prompt, history) = prompt_and_history_for_request(request.llm_request())?;
    let tool_state = Arc::new(RigToolExecutionState::default());
    let rig_tools = build_rig_tools_for_request(
        request,
        executor,
        cancellation.clone(),
        Arc::clone(&tool_state),
    );
    let hook = RigToolProgressHook::new(Arc::clone(&tool_state));
    let agent = if rig_tools.is_empty() {
        AgentBuilder::new(model).build()
    } else {
        AgentBuilder::new(model).tools(rig_tools).build()
    };
    let mut accumulator = RigAgentAccumulator::new(request.llm_request().model_id.clone());
    let request_started_at = Instant::now();

    let mut stream = tokio::select! {
        _ = cancellation.cancelled() => return Err(NativeAgentError::Cancelled),
        stream = agent
            .stream_chat(prompt, history)
            .multi_turn(MAX_AGENT_TOOL_ROUNDS)
            .with_hook(hook) => stream,
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
                    Arc::clone(&tool_state),
                    &mut accumulator,
                    on_progress,
                );
            }
            MultiTurnStreamItem::StreamUserItem(content) => {
                handle_streamed_user_content(
                    content,
                    Arc::clone(&tool_state),
                    &mut accumulator,
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
    tool_state: Arc<RigToolExecutionState>,
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
            let runtime_call = runtime_tool_call_from_rig(tool_call);
            tool_state.register_streamed_tool_call(internal_call_id, runtime_call.clone());
            accumulator.tool_calls.push(runtime_call.clone());
            on_progress(NativeAgentProgress::ToolExecutionStarted { call: runtime_call });
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
    tool_state: Arc<RigToolExecutionState>,
    accumulator: &mut RigAgentAccumulator,
    on_progress: &mut F,
) where
    F: FnMut(NativeAgentProgress),
{
    let StreamedUserContent::ToolResult {
        tool_result,
        internal_call_id,
    } = content;
    let fallback_call = RuntimeToolCall::new(
        tool_result
            .call_id
            .clone()
            .unwrap_or_else(|| tool_result.id.clone()),
        "tool",
        serde_json::json!({}),
    );
    let result = tool_state
        .take_completed_tool_result(&internal_call_id)
        .unwrap_or_else(|| {
            RuntimeToolResult::success(
                fallback_call.call_id.clone(),
                tool_result_text(&tool_result.content),
            )
        });
    let call = tool_state
        .take_streamed_tool_call(&internal_call_id)
        .unwrap_or(fallback_call);

    accumulator.tool_results.push(result.clone());
    on_progress(NativeAgentProgress::ToolExecutionFinished { call, result });
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

struct RigAgentAccumulator {
    content: String,
    final_content: Option<String>,
    reasoning_content: String,
    tool_calls: Vec<RuntimeToolCall>,
    tool_results: Vec<RuntimeToolResult>,
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
            tool_calls: Vec::new(),
            tool_results: Vec::new(),
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
                tool_calls: self.tool_calls,
                tool_results: self.tool_results,
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
