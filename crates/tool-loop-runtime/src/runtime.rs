use std::time::{Duration, Instant};

use provider_protocol::{
    Message, PromptRequest, ProviderClient, ProviderError, StreamEvent, StreamEventSink,
    ToolCall as AiToolCall, ToolDefinition as AiToolDefinition, ToolResult as AiToolResult,
};
use runtime_domain::{
    session::{ProviderRequestMetrics, RuntimeToolActivity, RuntimeToolActivityUpdate},
    token_count::StreamingTokenProgress,
};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    DefaultToolErrorFormatter, ProcessedToolError, SharedToolErrorFormatter,
    SharedToolPermissionHandler, ToolExecutor, ToolExecutorRegistry, ToolPermissionDecision,
    ToolPermissionPolicy, ToolPermissionRequest, ToolRegistry, ToolResult,
};

use crate::{
    activity::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result},
    error::ToolLoopError,
};

const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";
const TOOL_EXECUTION_INTERRUPTED: &str = "Tool execution interrupted";

/// `ToolLoopProgress` describes runtime progress and provider-context session deltas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolLoopProgress {
    ProviderTurnStarted,
    ProviderContextMessage { message: Message },
    OutputTokens { total_tokens: usize },
    InputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
    AssistantDelta { content: String },
    ReasoningDelta { content: String },
    ToolActivityStarted { activity: RuntimeToolActivity },
    ToolActivityUpdated { update: RuntimeToolActivityUpdate },
}

/// `ToolLoopResponse` is the final visible assistant output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolLoopResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
}

/// `ToolLoopCompletion` is returned when a runtime turn completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLoopCompletion {
    pub response: ToolLoopResponse,
    pub metrics: Option<ProviderRequestMetrics>,
    pub appended_messages: Vec<Message>,
}

/// `ToolLoopOptions` controls runtime-owned tool loop behavior.
#[derive(Clone)]
pub struct ToolLoopOptions {
    pub tool_max_turns: Option<usize>,
    pub permission_handler: Option<SharedToolPermissionHandler>,
    pub error_formatter: SharedToolErrorFormatter,
}

impl Default for ToolLoopOptions {
    fn default() -> Self {
        Self {
            tool_max_turns: None,
            permission_handler: None,
            error_formatter: std::sync::Arc::new(DefaultToolErrorFormatter),
        }
    }
}

/// `run_tool_loop` 负责执行 provider turn 与工具循环，直到本轮请求完成。
pub async fn run_tool_loop<C, F>(
    client: &C,
    mut request: PromptRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    options: ToolLoopOptions,
    mut on_progress: F,
) -> Result<ToolLoopCompletion, ToolLoopError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(ToolLoopProgress) + Send,
{
    if request.messages.is_empty() {
        return Err(ToolLoopError::EmptyPrompt);
    }
    if cancellation.is_cancelled() {
        return Err(ToolLoopError::Cancelled);
    }

    let tool_definitions = executor.definitions();
    request.tools = ai_tool_definitions_from_registry(&tool_definitions);
    let mut state = RuntimeTurnState::new(request.model.clone());
    let mut tool_turns = 0usize;
    let mut appended_messages = Vec::new();

    loop {
        let prompt = request.clone();
        let provider_response =
            stream_provider_turn(client, prompt, cancellation, &mut state, &mut on_progress)
                .await?;
        state.capture_turn_response(&provider_response);
        let tool_calls = provider_response.tool_calls.clone();
        if !provider_response.finish_reason.is_tool_call() || tool_calls.is_empty() {
            append_provider_context_message(
                provider_response.message,
                &mut appended_messages,
                &mut on_progress,
            );
            return Ok(state.finish_at(Instant::now(), appended_messages));
        }

        if let Some(max_turns) = options.tool_max_turns
            && tool_turns >= max_turns
        {
            return Err(ToolLoopError::ToolTurnLimit { max_turns });
        }
        state.mark_tool_call_turn();
        tool_turns = tool_turns.saturating_add(1);
        request.messages.push(provider_response.message.clone());
        append_provider_context_message(
            provider_response.message,
            &mut appended_messages,
            &mut on_progress,
        );

        let mut tool_result_batch = Vec::new();
        for call in tool_calls {
            let activity = runtime_tool_activity_from_call(&call, &tool_definitions);
            on_progress(ToolLoopProgress::ToolActivityStarted { activity });
            let execution = if cancellation.is_cancelled() {
                interrupted_tool_execution(&call)
            } else {
                execute_tool_call(
                    &call,
                    &executor,
                    &tool_definitions,
                    cancellation,
                    options.permission_handler.as_ref(),
                    &options.error_formatter,
                )
                .await
            };
            let update = runtime_tool_activity_update_from_result(
                &call,
                &execution.raw_result,
                execution.processed_error.as_ref(),
                &tool_definitions,
            );
            on_progress(ToolLoopProgress::ToolActivityUpdated { update });
            let tool_result_message = Message::tool_result(execution.provider_result);
            tool_result_batch.push((tool_result_message, execution.raw_result.terminate));
        }

        let should_terminate_after_batch = tool_result_batch
            .iter()
            .all(|(_, should_terminate)| *should_terminate);
        for (tool_result_message, _) in tool_result_batch {
            if should_terminate_after_batch {
                append_provider_context_message(
                    tool_result_message,
                    &mut appended_messages,
                    &mut on_progress,
                );
                continue;
            }
            let provider_result_content = tool_result_message
                .first_tool_result()
                .expect("runtime-created tool result messages should contain a tool result")
                .content
                .as_str();
            state.observe_tool_result_input(
                provider_result_content,
                Instant::now(),
                &mut on_progress,
            );
            request.messages.push(tool_result_message.clone());
            append_provider_context_message(
                tool_result_message,
                &mut appended_messages,
                &mut on_progress,
            );
        }
        if cancellation.is_cancelled() {
            return Err(ToolLoopError::Cancelled);
        }
        if should_terminate_after_batch {
            return Ok(state.finish_at(Instant::now(), appended_messages));
        }
    }
}

async fn stream_provider_turn<C, F>(
    client: &C,
    request: PromptRequest,
    cancellation: &CancellationToken,
    state: &mut RuntimeTurnState,
    on_progress: &mut F,
) -> Result<provider_protocol::PromptResponse, ToolLoopError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(ToolLoopProgress) + Send,
{
    if cancellation.is_cancelled() {
        return Err(ToolLoopError::Cancelled);
    }
    on_progress(ToolLoopProgress::ProviderTurnStarted);
    state.mark_request_started(Instant::now());
    let mut provider_response = None;
    let result = {
        let mut sink = RuntimeStreamSink {
            state,
            on_progress,
            provider_response: &mut provider_response,
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(ToolLoopError::Cancelled),
            result = client.stream_prompt(request, &mut sink) => result,
        }
    };

    match result {
        Ok(response) => Ok(provider_response.unwrap_or(response)),
        Err(ProviderError::Transport(message)) if cancellation.is_cancelled() => {
            let _ = message;
            Err(ToolLoopError::Cancelled)
        }
        Err(error) => Err(error.into()),
    }
}

fn append_provider_context_message<F>(
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
    provider_response: &'a mut Option<provider_protocol::PromptResponse>,
}

impl<F> StreamEventSink for RuntimeStreamSink<'_, F>
where
    F: FnMut(ToolLoopProgress),
{
    fn emit(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::MessageStarted => self.state.mark_request_started(Instant::now()),
            StreamEvent::TextDelta(content) => {
                self.state
                    .observe_content_chunk(&content, Instant::now(), self.on_progress)
            }
            StreamEvent::ReasoningDelta(content) => {
                self.state
                    .observe_reasoning_chunk(&content, Instant::now(), self.on_progress)
            }
            StreamEvent::UsageUpdated(usage) => {
                if let Some(output_tokens) = usage.output_tokens {
                    self.state.final_output_tokens = Some(output_tokens as usize);
                }
            }
            StreamEvent::MessageCompleted(response) => {
                *self.provider_response = Some(response);
            }
            StreamEvent::ToolCallStarted { .. }
            | StreamEvent::ToolCallArgumentsDelta { .. }
            | StreamEvent::ToolCallCompleted { .. } => {}
        }
    }
}

struct ToolExecution {
    raw_result: ToolResult,
    provider_result: AiToolResult,
    processed_error: Option<ProcessedToolError>,
}

fn interrupted_tool_execution(call: &AiToolCall) -> ToolExecution {
    let processed_error =
        ProcessedToolError::new(TOOL_EXECUTION_INTERRUPTED, TOOL_EXECUTION_INTERRUPTED);
    ToolExecution {
        raw_result: ToolResult::error(call.call_id.clone(), TOOL_EXECUTION_INTERRUPTED),
        provider_result: AiToolResult::error(
            call.call_id.clone(),
            call.name.clone(),
            processed_error.assistant_message.clone(),
            None,
        ),
        processed_error: Some(processed_error),
    }
}

async fn execute_tool_call(
    call: &AiToolCall,
    executor: &ToolExecutorRegistry,
    tool_definitions: &ToolRegistry,
    cancellation: &CancellationToken,
    permission_handler: Option<&SharedToolPermissionHandler>,
    error_formatter: &SharedToolErrorFormatter,
) -> ToolExecution {
    let runtime_call = tool_runtime::ToolCall::new(
        call.call_id.clone(),
        call.name.clone(),
        call.arguments.clone(),
    );

    let raw_result = match authorize_tool_call(
        &runtime_call,
        tool_definitions,
        cancellation,
        permission_handler,
    )
    .await
    {
        Some(message) => ToolResult::error(call.call_id.clone(), message),
        None => executor.execute_tool(runtime_call, cancellation).await,
    };

    let processed_error = raw_result
        .is_error
        .then(|| error_formatter.format_tool_error(&call.name, &raw_result.content));
    let provider_content = processed_error
        .as_ref()
        .map(|processed| processed.assistant_message.clone())
        .unwrap_or_else(|| raw_result.content.clone());
    let provider_result = if raw_result.is_error {
        AiToolResult::error(
            call.call_id.clone(),
            call.name.clone(),
            provider_content,
            raw_result.details.clone(),
        )
    } else {
        AiToolResult::success(
            call.call_id.clone(),
            call.name.clone(),
            provider_content,
            raw_result.details.clone(),
        )
    };

    ToolExecution {
        raw_result,
        provider_result,
        processed_error,
    }
}

async fn authorize_tool_call(
    call: &tool_runtime::ToolCall,
    tool_definitions: &ToolRegistry,
    cancellation: &CancellationToken,
    permission_handler: Option<&SharedToolPermissionHandler>,
) -> Option<String> {
    let definition = tool_definitions.definition(&call.name).cloned()?;

    match definition.permission_policy {
        ToolPermissionPolicy::Always => None,
        ToolPermissionPolicy::Never => Some(format!(
            "{TOOL_PERMISSION_DENIED}: {} is not allowed",
            definition.name
        )),
        ToolPermissionPolicy::Ask => {
            let Some(permission_handler) = permission_handler else {
                return Some(format!(
                    "{TOOL_PERMISSION_DENIED}: {} requires approval",
                    definition.name
                ));
            };
            match permission_handler
                .request_permission(
                    ToolPermissionRequest::new(call.clone(), definition),
                    cancellation,
                )
                .await
            {
                ToolPermissionDecision::Allow => None,
                ToolPermissionDecision::Deny { message } => Some(message),
            }
        }
    }
}

fn ai_tool_definitions_from_registry(registry: &ToolRegistry) -> Vec<AiToolDefinition> {
    registry
        .definitions()
        .map(|definition| {
            AiToolDefinition::new(
                definition.name.clone(),
                definition.description.clone().unwrap_or_default(),
                definition
                    .input_schema
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
            )
        })
        .collect()
}

struct RuntimeTurnState {
    content: String,
    final_content: Option<String>,
    reasoning_content: String,
    output_progress: StreamingTokenProgress,
    input_progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
    request_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    final_output_tokens: Option<usize>,
    saw_tool_call_turn: bool,
}

impl RuntimeTurnState {
    fn new(model_id: String) -> Self {
        Self {
            content: String::new(),
            final_content: None,
            reasoning_content: String::new(),
            output_progress: StreamingTokenProgress::new(model_id.clone()),
            input_progress: StreamingTokenProgress::new(model_id),
            is_thinking: false,
            reasoning_started_at: None,
            reasoning_finished_at: None,
            request_started_at: None,
            first_token_at: None,
            final_output_tokens: None,
            saw_tool_call_turn: false,
        }
    }

    fn mark_request_started(&mut self, now: Instant) {
        self.request_started_at.get_or_insert(now);
    }

    fn capture_turn_response(&mut self, response: &provider_protocol::PromptResponse) {
        self.final_content = Some(response.message.text_content());
    }

    fn mark_tool_call_turn(&mut self) {
        self.saw_tool_call_turn = true;
    }

    fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(ToolLoopProgress::Thinking { is_thinking: false });
        }
        self.content.push_str(content);
        on_progress(ToolLoopProgress::AssistantDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(ToolLoopProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        on_progress(ToolLoopProgress::ReasoningDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_token_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if let Some(total_tokens) = self.output_progress.observe_delta(content, now) {
            on_progress(ToolLoopProgress::OutputTokens { total_tokens });
        }
    }

    fn observe_tool_result_input(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.input_progress, content, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::InputTokens { total_tokens });
    }

    fn finish_at(
        mut self,
        finished_at: Instant,
        appended_messages: Vec<Message>,
    ) -> ToolLoopCompletion {
        if self.is_thinking {
            self.is_thinking = false;
        }
        let _ = self.output_progress.flush(finished_at);
        let metrics = self.performance_metrics(finished_at);
        let reasoning_content = trim_outer_blank_lines(&self.reasoning_content);
        let reasoning_duration = self.reasoning_duration();
        let content = self
            .final_content
            .unwrap_or(self.content)
            .trim_end()
            .to_string();
        ToolLoopCompletion {
            response: ToolLoopResponse {
                content,
                reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
                reasoning_duration,
            },
            metrics,
            appended_messages,
        }
    }

    fn performance_metrics(&self, finished_at: Instant) -> Option<ProviderRequestMetrics> {
        let request_started_at = self.request_started_at?;
        let first_token_at = self.first_token_at?;
        Some(ProviderRequestMetrics {
            latency: first_token_at.saturating_duration_since(request_started_at),
            output_tokens: if self.saw_tool_call_turn {
                self.output_progress.total_tokens()
            } else {
                self.final_output_tokens
                    .unwrap_or_else(|| self.output_progress.total_tokens())
            },
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

fn observe_complete_token_total(
    progress: &mut StreamingTokenProgress,
    content: &str,
    now: Instant,
) -> Option<usize> {
    if content.is_empty() {
        return None;
    }

    progress
        .observe_delta(content, now)
        .or_else(|| progress.flush(now))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use provider_protocol::{
        Message, MessageRole, ModelDescriptor, PromptRequest, PromptResponse, ProviderCapabilities,
        ProviderClient, ProviderError, ProviderFuture, StreamEvent, StreamEventSink, TokenUsage,
        ToolCall,
    };
    use runtime_domain::token_count::StreamingTokenProgress;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::{
        Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionFuture,
        ToolExecutorRegistry, ToolKind, ToolPermissionPolicy, ToolResult,
    };

    use super::{ToolLoopOptions, ToolLoopProgress, run_tool_loop};

    struct FakeProvider {
        calls: Mutex<usize>,
    }

    impl ProviderClient for FakeProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
            Box::pin(async move {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
                if *calls == 1 {
                    let call = ToolCall::new("call-1", "echo", serde_json::json!({ "text": "hi" }));
                    sink.emit(StreamEvent::TextDelta("checking".to_string()));
                    let response = PromptResponse::new(
                        Message::assistant_with_tool_calls(
                            "checking".to_string(),
                            vec![call.clone()],
                        ),
                        provider_protocol::FinishReason::ToolCalls,
                        None,
                        vec![call],
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                }
            })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct UsageProvider;

    impl ProviderClient for UsageProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
            Box::pin(async move {
                sink.emit(StreamEvent::MessageStarted);
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(3),
                    None,
                )));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(5),
                    None,
                )));
                let response = PromptResponse::new(
                    Message::text(MessageRole::Assistant, "done"),
                    provider_protocol::FinishReason::Stop,
                    Some(TokenUsage::new(None, Some(5), None)),
                    Vec::new(),
                );
                sink.emit(StreamEvent::MessageCompleted(response.clone()));
                Ok(response)
            })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct DelayedFirstTokenProvider;

    impl ProviderClient for DelayedFirstTokenProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                sink.emit(StreamEvent::MessageStarted);
                tokio::time::sleep(Duration::from_millis(40)).await;
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = PromptResponse::new(
                    Message::text(MessageRole::Assistant, "done"),
                    provider_protocol::FinishReason::Stop,
                    None,
                    Vec::new(),
                );
                sink.emit(StreamEvent::MessageCompleted(response.clone()));
                Ok(response)
            })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct ToolLoopUsageProvider;

    impl ProviderClient for ToolLoopUsageProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = request
                    .messages
                    .iter()
                    .any(|message| message.first_tool_result().is_some());
                sink.emit(StreamEvent::MessageStarted);
                if !has_tool_result {
                    let call = ToolCall::new("call-1", "echo", serde_json::json!({ "text": "hi" }));
                    sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(20),
                        None,
                    )));
                    let response = PromptResponse::new(
                        Message::assistant_with_tool_calls(
                            "hello world".to_string(),
                            vec![call.clone()],
                        ),
                        provider_protocol::FinishReason::ToolCalls,
                        Some(TokenUsage::new(None, Some(20), None)),
                        vec![call],
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(5),
                        None,
                    )));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        Some(TokenUsage::new(None, Some(5), None)),
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                }
            })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct MultiToolBatchProvider {
        calls: Mutex<usize>,
        request_messages: Mutex<Vec<Vec<Message>>>,
    }

    impl MultiToolBatchProvider {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
                request_messages: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProviderClient for MultiToolBatchProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
            Box::pin(async move {
                self.request_messages
                    .lock()
                    .expect("request messages lock should not poison")
                    .push(request.messages.clone());
                let call_count = {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    *calls
                };
                sink.emit(StreamEvent::MessageStarted);
                if call_count == 1 {
                    let terminating_call = ToolCall::new(
                        "call-terminate",
                        "echo",
                        serde_json::json!({ "terminate": true }),
                    );
                    let continuing_call = ToolCall::new(
                        "call-continue",
                        "echo",
                        serde_json::json!({ "terminate": false }),
                    );
                    let calls = vec![terminating_call.clone(), continuing_call.clone()];
                    sink.emit(StreamEvent::TextDelta("checking".to_string()));
                    let response = PromptResponse::new(
                        Message::assistant_with_tool_calls("checking".to_string(), calls.clone()),
                        provider_protocol::FinishReason::ToolCalls,
                        None,
                        calls,
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                }
            })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct ConditionalTerminatingTool;

    impl Tool for ConditionalTerminatingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Other)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move {
                let should_terminate = call
                    .arguments
                    .get("terminate")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let mut result = ToolResult::success(call.call_id.clone(), call.call_id);
                result.terminate = should_terminate;
                result
            })
        }
    }

    struct TerminatingTool;

    impl Tool for TerminatingTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Other)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move {
                let mut result = ToolResult::success(call.call_id, "terminate here");
                result.terminate = true;
                result
            })
        }
    }

    struct EchoTool;

    impl Tool for EchoTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Other)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
        }
    }

    #[tokio::test]
    async fn runtime_executes_tool_calls_and_continues_until_final_text() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect("runtime should complete");

        assert_eq!(completion.response.content, "done");
        assert!(events.iter().any(|event| {
            matches!(event, ToolLoopProgress::AssistantDelta { content } if content == "checking")
        }));
        assert!(events.iter().any(|event| {
            matches!(event, ToolLoopProgress::ToolActivityStarted { activity } if activity.title == "Echo")
        }));
        assert_eq!(*provider.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn provider_context_messages_emit_before_follow_up_provider_failure() {
        struct FailingFollowUpProvider {
            calls: Mutex<usize>,
        }

        impl ProviderClient for FailingFollowUpProvider {
            fn stream_prompt<'a>(
                &'a self,
                _request: PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    if *calls == 1 {
                        let call =
                            ToolCall::new("call-1", "echo", serde_json::json!({ "text": "hi" }));
                        let response = PromptResponse::new(
                            Message::assistant_with_tool_calls(String::new(), vec![call.clone()]),
                            provider_protocol::FinishReason::ToolCalls,
                            None,
                            vec![call],
                        );
                        sink.emit(StreamEvent::MessageCompleted(response.clone()));
                        Ok(response)
                    } else {
                        Err(ProviderError::Transport("connection dropped".to_string()))
                    }
                })
            }

            fn list_models<'a>(
                &'a self,
            ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities::chat_completions()
            }
        }

        let provider = FailingFollowUpProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let error = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect_err("follow-up provider error should fail the turn");

        assert!(matches!(
            error,
            super::ToolLoopError::Provider(ProviderError::Transport(_))
        ));
        let committed_roles = events
            .iter()
            .filter_map(|event| match event {
                ToolLoopProgress::ProviderContextMessage { message } => Some(message.role),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            committed_roles,
            vec![MessageRole::Assistant, MessageRole::Tool]
        );
        assert_eq!(*provider.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn cancelled_tool_batch_emits_matching_tool_results_before_stopping() {
        struct TwoToolProvider {
            calls: Mutex<usize>,
        }

        impl ProviderClient for TwoToolProvider {
            fn stream_prompt<'a>(
                &'a self,
                _request: PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    let first = ToolCall::new("call-1", "cancel_once", serde_json::json!({}));
                    let second = ToolCall::new("call-2", "cancel_once", serde_json::json!({}));
                    let response = PromptResponse::new(
                        Message::assistant_with_tool_calls(
                            String::new(),
                            vec![first.clone(), second.clone()],
                        ),
                        provider_protocol::FinishReason::ToolCalls,
                        None,
                        vec![first, second],
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                })
            }

            fn list_models<'a>(
                &'a self,
            ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities::chat_completions()
            }
        }

        struct CancelOnceTool(Arc<Mutex<usize>>);

        impl Tool for CancelOnceTool {
            fn definition(&self) -> ToolDefinition {
                ToolDefinition::new("cancel_once")
                    .with_label("Cancel Once")
                    .with_kind(ToolKind::Other)
                    .with_permission_policy(ToolPermissionPolicy::Always)
            }

            fn execute<'a>(
                &'a self,
                call: RuntimeToolCall,
                cancellation: &'a CancellationToken,
            ) -> ToolExecutionFuture<'a> {
                *self.0.lock().expect("execution lock should not poison") += 1;
                cancellation.cancel();
                Box::pin(async move { ToolResult::success(call.call_id, "first result") })
            }
        }

        let provider = TwoToolProvider {
            calls: Mutex::new(0),
        };
        let executions = Arc::new(Mutex::new(0));
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(CancelOnceTool(Arc::clone(&executions)));
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "call tools")],
        );
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let error = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect_err("cancellation should stop before a follow-up provider turn");

        assert!(matches!(error, super::ToolLoopError::Cancelled));
        assert_eq!(*provider.calls.lock().unwrap(), 1);
        assert_eq!(*executions.lock().unwrap(), 1);
        let committed_messages = events
            .iter()
            .filter_map(|event| match event {
                ToolLoopProgress::ProviderContextMessage { message } => Some(message),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            committed_messages
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            vec![MessageRole::Assistant, MessageRole::Tool, MessageRole::Tool]
        );
        let interrupted_result = committed_messages[2]
            .first_tool_result()
            .expect("interrupted tool should have a synthetic result");
        assert!(interrupted_result.is_error);
        assert_eq!(interrupted_result.content, "Tool execution interrupted");
    }

    #[tokio::test]
    async fn usage_updates_replace_output_token_metric() {
        let provider = UsageProvider;
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "count tokens")],
        );
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            ToolExecutorRegistry::new(),
            &cancellation,
            ToolLoopOptions::default(),
            |_| {},
        )
        .await
        .expect("runtime should complete");

        assert_eq!(
            completion
                .metrics
                .expect("metrics should use provider usage")
                .output_tokens,
            5
        );
    }

    #[tokio::test]
    async fn request_latency_starts_before_first_stream_frame() {
        let provider = DelayedFirstTokenProvider;
        let request = PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "hello")]);
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            ToolExecutorRegistry::new(),
            &cancellation,
            ToolLoopOptions::default(),
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let metrics = completion.metrics.expect("metrics should be recorded");
        assert!(
            metrics.latency >= Duration::from_millis(75),
            "latency should include time before the first SSE frame arrives: {:?}",
            metrics.latency
        );
    }

    #[tokio::test]
    async fn tool_result_input_tokens_emit_progress() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let _ = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect("runtime should complete");

        assert!(events.iter().any(|event| {
            matches!(event, ToolLoopProgress::InputTokens { total_tokens } if *total_tokens > 0)
        }));
    }

    #[tokio::test]
    async fn final_metrics_flush_pending_output_tokens_without_provider_usage() {
        struct SplitDeltaProvider;

        impl ProviderClient for SplitDeltaProvider {
            fn stream_prompt<'a>(
                &'a self,
                _request: PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
                Box::pin(async move {
                    sink.emit(StreamEvent::MessageStarted);
                    sink.emit(StreamEvent::TextDelta("hello".to_string()));
                    sink.emit(StreamEvent::TextDelta(" world".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "hello world"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                })
            }

            fn list_models<'a>(
                &'a self,
            ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
                Box::pin(async { Ok(Vec::new()) })
            }

            fn capabilities(&self) -> ProviderCapabilities {
                ProviderCapabilities::chat_completions()
            }
        }

        let provider = SplitDeltaProvider;
        let request = PromptRequest::new("gpt-4o", vec![Message::text(MessageRole::User, "hello")]);
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            ToolExecutorRegistry::new(),
            &cancellation,
            ToolLoopOptions::default(),
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let metrics = completion.metrics.expect("metrics should be recorded");
        assert!(
            metrics.output_tokens >= 2,
            "final metrics should include the last throttled output delta: {}",
            metrics.output_tokens
        );
    }

    #[tokio::test]
    async fn tool_loop_metrics_ignore_last_turn_usage_and_keep_cumulative_visible_output() {
        let provider = ToolLoopUsageProvider;
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request = PromptRequest::new(
            "gpt-4o",
            vec![Message::text(MessageRole::User, "call echo")],
        );
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let metrics = completion.metrics.expect("metrics should be recorded");
        let started_at = Instant::now();
        let mut progress = StreamingTokenProgress::new("gpt-4o");
        let _ = progress.observe_delta("hello world", started_at);
        let _ = progress.observe_delta("done", started_at + Duration::from_millis(200));
        let _ = progress.flush(started_at + Duration::from_millis(400));
        let expected = progress.total_tokens();

        assert_eq!(
            metrics.output_tokens, expected,
            "tool-loop throughput should use cumulative visible output instead of last-turn provider usage"
        );
    }

    #[tokio::test]
    async fn mixed_terminating_tool_batch_executes_all_tools_and_continues() {
        let provider = MultiToolBatchProvider::new();
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(ConditionalTerminatingTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "call both tools")],
        );
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect("runtime should complete");

        assert_eq!(completion.response.content, "done");
        assert_eq!(*provider.calls.lock().unwrap(), 2);
        assert_eq!(
            completion
                .appended_messages
                .iter()
                .map(|message| message.role)
                .collect::<Vec<_>>(),
            vec![
                MessageRole::Assistant,
                MessageRole::Tool,
                MessageRole::Tool,
                MessageRole::Assistant,
            ]
        );
        assert_eq!(
            completion.appended_messages[1]
                .first_tool_result()
                .expect("first tool result should be preserved")
                .call_id,
            "call-terminate"
        );
        assert_eq!(
            completion.appended_messages[2]
                .first_tool_result()
                .expect("second tool result should be preserved")
                .call_id,
            "call-continue"
        );

        let request_messages = provider
            .request_messages
            .lock()
            .expect("request messages lock should not poison");
        let second_request_tool_result_ids = request_messages[1]
            .iter()
            .filter_map(|message| message.first_tool_result())
            .map(|result| result.call_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            second_request_tool_result_ids,
            vec!["call-terminate", "call-continue"]
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ToolLoopProgress::InputTokens { .. })),
            "non-terminating batches should send tool results into the follow-up provider turn"
        );
    }

    #[tokio::test]
    async fn terminating_tool_result_does_not_emit_input_tokens() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(TerminatingTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect("runtime should complete");

        assert_eq!(completion.appended_messages.len(), 2);
        assert_eq!(completion.appended_messages[0].role, MessageRole::Assistant);
        assert_eq!(completion.appended_messages[1].role, MessageRole::Tool);
        assert_eq!(
            completion.appended_messages[0].tool_calls()[0].call_id,
            "call-1"
        );
        assert_eq!(
            completion.appended_messages[1]
                .first_tool_result()
                .expect("terminating tool result should be preserved")
                .call_id,
            "call-1"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ToolLoopProgress::InputTokens { .. })),
            "terminating tool results should not pretend to send provider-context input"
        );
    }

    #[tokio::test]
    async fn never_permission_returns_failed_tool_update_without_executing() {
        struct DeniedTool(Arc<Mutex<usize>>);

        impl Tool for DeniedTool {
            fn definition(&self) -> ToolDefinition {
                ToolDefinition::new("denied")
                    .with_label("Denied")
                    .with_permission_policy(ToolPermissionPolicy::Never)
            }

            fn execute<'a>(
                &'a self,
                call: RuntimeToolCall,
                _cancellation: &'a CancellationToken,
            ) -> ToolExecutionFuture<'a> {
                *self.0.lock().unwrap() += 1;
                Box::pin(async move { ToolResult::success(call.call_id, "should not run") })
            }
        }

        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let executions = Arc::new(Mutex::new(0));
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(DeniedTool(Arc::clone(&executions)));
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "call denied")],
        );
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let _ = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                tool_max_turns: Some(1),
                ..ToolLoopOptions::default()
            },
            |event| events.push(event),
        )
        .await;

        assert_eq!(*executions.lock().unwrap(), 0);
        assert!(events.iter().any(|event| {
            matches!(event, ToolLoopProgress::ToolActivityUpdated { update }
                if matches!(update.status, Some(runtime_domain::session::RuntimeToolActivityStatus::Failed)))
        }));
    }
}
