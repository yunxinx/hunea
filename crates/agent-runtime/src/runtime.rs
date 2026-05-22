use std::time::{Duration, Instant};

use mo_ai_core::{
    Message, PromptRequest, ProviderClient, ProviderError, StreamEvent, StreamEventSink,
    ToolCall as AiToolCall, ToolDefinition as AiToolDefinition, ToolResult as AiToolResult,
};
use mo_core::{
    session::{NativeLlmPerformanceMetrics, RuntimeToolActivity, RuntimeToolActivityUpdate},
    token_count::StreamingTokenProgress,
};
use mo_tools::{
    DefaultToolErrorFormatter, ProcessedToolError, SharedToolErrorFormatter,
    SharedToolPermissionHandler, ToolExecutor, ToolExecutorRegistry, ToolPermissionDecision,
    ToolPermissionPolicy, ToolPermissionRequest, ToolRegistry, ToolResult,
};
use tokio_util::sync::CancellationToken;

use crate::{
    activity::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result},
    error::AgentRuntimeError,
};

const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";

/// `AgentRuntimeProgress` describes UI-consumable runtime progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRuntimeProgress {
    OutputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
    AssistantDelta { content: String },
    ReasoningDelta { content: String },
    ToolActivityStarted { activity: RuntimeToolActivity },
    ToolActivityUpdated { update: RuntimeToolActivityUpdate },
}

/// `AgentRuntimeResponse` is the final visible assistant output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRuntimeResponse {
    pub content: String,
    pub reasoning_content: Option<String>,
    pub reasoning_duration: Option<Duration>,
}

/// `AgentRuntimeCompletion` is returned when a runtime turn completes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntimeCompletion {
    pub response: AgentRuntimeResponse,
    pub metrics: Option<NativeLlmPerformanceMetrics>,
}

/// `AgentRuntimeOptions` controls runtime-owned tool loop behavior.
#[derive(Clone)]
pub struct AgentRuntimeOptions {
    pub tool_max_turns: Option<usize>,
    pub permission_handler: Option<SharedToolPermissionHandler>,
    pub error_formatter: SharedToolErrorFormatter,
}

impl Default for AgentRuntimeOptions {
    fn default() -> Self {
        Self {
            tool_max_turns: None,
            permission_handler: None,
            error_formatter: std::sync::Arc::new(DefaultToolErrorFormatter),
        }
    }
}

/// `run_agent_runtime` runs provider turns and Lumos-owned tool loop until completion.
pub async fn run_agent_runtime<C, F>(
    client: &C,
    mut request: PromptRequest,
    executor: ToolExecutorRegistry,
    cancellation: &CancellationToken,
    options: AgentRuntimeOptions,
    mut on_progress: F,
) -> Result<AgentRuntimeCompletion, AgentRuntimeError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(AgentRuntimeProgress) + Send,
{
    if request.messages.is_empty() {
        return Err(AgentRuntimeError::EmptyPrompt);
    }
    if cancellation.is_cancelled() {
        return Err(AgentRuntimeError::Cancelled);
    }

    let tool_definitions = executor.definitions();
    request.tools = ai_tool_definitions_from_registry(&tool_definitions);
    let mut state = RuntimeTurnState::new(request.model.clone());
    let mut tool_turns = 0usize;

    loop {
        let prompt = request.clone();
        let provider_response =
            stream_provider_turn(client, prompt, cancellation, &mut state, &mut on_progress)
                .await?;
        state.capture_turn_response(&provider_response);
        let tool_calls = provider_response.tool_calls.clone();
        if !provider_response.finish_reason.is_tool_call() || tool_calls.is_empty() {
            return Ok(state.finish_at(Instant::now()));
        }

        if let Some(max_turns) = options.tool_max_turns
            && tool_turns >= max_turns
        {
            return Err(AgentRuntimeError::ToolTurnLimit { max_turns });
        }
        tool_turns = tool_turns.saturating_add(1);
        request.messages.push(provider_response.message);

        for call in tool_calls {
            let activity = runtime_tool_activity_from_call(&call, &tool_definitions);
            on_progress(AgentRuntimeProgress::ToolActivityStarted { activity });
            let execution = execute_tool_call(
                &call,
                &executor,
                &tool_definitions,
                cancellation,
                options.permission_handler.as_ref(),
                &options.error_formatter,
            )
            .await;
            let update = runtime_tool_activity_update_from_result(
                &call,
                &execution.raw_result,
                execution.processed_error.as_ref(),
                &tool_definitions,
            );
            on_progress(AgentRuntimeProgress::ToolActivityUpdated { update });
            request
                .messages
                .push(Message::tool_result(execution.provider_result));
            if execution.raw_result.terminate {
                return Ok(state.finish_at(Instant::now()));
            }
        }
    }
}

async fn stream_provider_turn<C, F>(
    client: &C,
    request: PromptRequest,
    cancellation: &CancellationToken,
    state: &mut RuntimeTurnState,
    on_progress: &mut F,
) -> Result<mo_ai_core::PromptResponse, AgentRuntimeError>
where
    C: ProviderClient + ?Sized,
    F: FnMut(AgentRuntimeProgress) + Send,
{
    let mut provider_response = None;
    let result = {
        let mut sink = RuntimeStreamSink {
            state,
            on_progress,
            provider_response: &mut provider_response,
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(AgentRuntimeError::Cancelled),
            result = client.stream_prompt(request, &mut sink) => result,
        }
    };

    match result {
        Ok(response) => Ok(provider_response.unwrap_or(response)),
        Err(ProviderError::Transport(message)) if cancellation.is_cancelled() => {
            let _ = message;
            Err(AgentRuntimeError::Cancelled)
        }
        Err(error) => Err(error.into()),
    }
}

struct RuntimeStreamSink<'a, F>
where
    F: FnMut(AgentRuntimeProgress),
{
    state: &'a mut RuntimeTurnState,
    on_progress: &'a mut F,
    provider_response: &'a mut Option<mo_ai_core::PromptResponse>,
}

impl<F> StreamEventSink for RuntimeStreamSink<'_, F>
where
    F: FnMut(AgentRuntimeProgress),
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

async fn execute_tool_call(
    call: &AiToolCall,
    executor: &ToolExecutorRegistry,
    tool_definitions: &ToolRegistry,
    cancellation: &CancellationToken,
    permission_handler: Option<&SharedToolPermissionHandler>,
    error_formatter: &SharedToolErrorFormatter,
) -> ToolExecution {
    let runtime_call = mo_tools::ToolCall::new(
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
    call: &mo_tools::ToolCall,
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
    progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
    request_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    final_output_tokens: Option<usize>,
}

impl RuntimeTurnState {
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
        self.request_started_at.get_or_insert(now);
    }

    fn capture_turn_response(&mut self, response: &mo_ai_core::PromptResponse) {
        self.final_content = Some(response.message.text_content());
    }

    fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(AgentRuntimeProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(AgentRuntimeProgress::Thinking { is_thinking: false });
        }
        self.content.push_str(content);
        on_progress(AgentRuntimeProgress::AssistantDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(AgentRuntimeProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(AgentRuntimeProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        on_progress(AgentRuntimeProgress::ReasoningDelta {
            content: content.to_string(),
        });
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_token_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(AgentRuntimeProgress),
    ) {
        if let Some(total_tokens) = self.progress.observe_delta(content, now) {
            on_progress(AgentRuntimeProgress::OutputTokens { total_tokens });
        }
    }

    fn finish_at(mut self, finished_at: Instant) -> AgentRuntimeCompletion {
        if self.is_thinking {
            self.is_thinking = false;
        }
        let metrics = self.performance_metrics(finished_at);
        let reasoning_content = trim_outer_blank_lines(&self.reasoning_content);
        let reasoning_duration = self.reasoning_duration();
        let content = self
            .final_content
            .unwrap_or(self.content)
            .trim_end()
            .to_string();
        AgentRuntimeCompletion {
            response: AgentRuntimeResponse {
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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mo_ai_core::{
        Message, MessageRole, ModelDescriptor, PromptRequest, PromptResponse, ProviderCapabilities,
        ProviderClient, ProviderError, ProviderFuture, StreamEvent, StreamEventSink, TokenUsage,
        ToolCall,
    };
    use mo_tools::{
        Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionFuture,
        ToolExecutorRegistry, ToolKind, ToolPermissionPolicy, ToolResult,
    };
    use tokio_util::sync::CancellationToken;

    use super::{AgentRuntimeOptions, AgentRuntimeProgress, run_agent_runtime};

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
                        mo_ai_core::FinishReason::ToolCalls,
                        None,
                        vec![call],
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        mo_ai_core::FinishReason::Stop,
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
                    mo_ai_core::FinishReason::Stop,
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

        let completion = run_agent_runtime(
            &provider,
            request,
            executor,
            &cancellation,
            AgentRuntimeOptions::default(),
            |event| events.push(event),
        )
        .await
        .expect("runtime should complete");

        assert_eq!(completion.response.content, "done");
        assert!(events.iter().any(|event| {
            matches!(event, AgentRuntimeProgress::AssistantDelta { content } if content == "checking")
        }));
        assert!(events.iter().any(|event| {
            matches!(event, AgentRuntimeProgress::ToolActivityStarted { activity } if activity.title == "Echo")
        }));
        assert_eq!(*provider.calls.lock().unwrap(), 2);
    }

    #[tokio::test]
    async fn usage_updates_replace_output_token_metric() {
        let provider = UsageProvider;
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "count tokens")],
        );
        let cancellation = CancellationToken::new();

        let completion = run_agent_runtime(
            &provider,
            request,
            ToolExecutorRegistry::new(),
            &cancellation,
            AgentRuntimeOptions::default(),
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

        let _ = run_agent_runtime(
            &provider,
            request,
            executor,
            &cancellation,
            AgentRuntimeOptions {
                tool_max_turns: Some(1),
                ..AgentRuntimeOptions::default()
            },
            |event| events.push(event),
        )
        .await;

        assert_eq!(*executions.lock().unwrap(), 0);
        assert!(events.iter().any(|event| {
            matches!(event, AgentRuntimeProgress::ToolActivityUpdated { update }
                if matches!(update.status, Some(mo_core::session::RuntimeToolActivityStatus::Failed)))
        }));
    }
}
