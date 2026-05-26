use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use provider_protocol::{
    Message, PromptRequest, ProviderClient, ProviderError, StreamEvent, StreamEventSink,
    ToolCall as AiToolCall, ToolDefinition as AiToolDefinition, ToolResult as AiToolResult,
};
use runtime_domain::{
    session::{
        ManagedSearchTool, ProviderRequestMetrics, RuntimeTerminalExitStatus,
        RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityUpdate,
    },
    token_count::StreamingTokenProgress,
};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    DefaultToolErrorFormatter, ProcessedToolError, SharedToolErrorFormatter,
    SharedToolPermissionHandler, ToolExecutionContext, ToolExecutor, ToolExecutorRegistry,
    ToolKind, ToolPermissionDecision, ToolPermissionFileSnapshot, ToolPermissionPolicy,
    ToolPermissionPreview, ToolPermissionRequest, ToolProgress, ToolProgressSink, ToolRegistry,
    ToolResult, ToolTerminalExitStatus, ToolTerminalSnapshot,
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
    SystemMessage { message: String },
    ProviderTurnStarted,
    ProviderContextMessage { message: Message },
    OutputTokens { total_tokens: usize },
    InputTokens { total_tokens: usize },
    Thinking { is_thinking: bool },
    AssistantDelta { content: String },
    ReasoningDelta { content: String },
    ToolActivityStarted { activity: RuntimeToolActivity },
    ToolActivityUpdated { update: RuntimeToolActivityUpdate },
    TerminalUpdated { snapshot: RuntimeTerminalSnapshot },
    ManagedSearchToolAuthorization { tool: ManagedSearchTool },
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

/// `ToolLoopClock` 抽象 runtime 计时来源，方便测试审批等待时间剔除逻辑。
#[derive(Clone)]
pub struct ToolLoopClock {
    now: std::sync::Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl Default for ToolLoopClock {
    fn default() -> Self {
        Self {
            now: std::sync::Arc::new(Instant::now),
        }
    }
}

impl ToolLoopClock {
    /// `new` 创建自定义计时来源。
    pub fn new(now: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        Self {
            now: std::sync::Arc::new(now),
        }
    }

    fn now(&self) -> Instant {
        (self.now)()
    }
}

/// `ToolLoopOptions` controls runtime-owned tool loop behavior.
#[derive(Clone)]
pub struct ToolLoopOptions {
    pub tool_max_turns: Option<usize>,
    pub permission_handler: Option<SharedToolPermissionHandler>,
    pub error_formatter: SharedToolErrorFormatter,
    pub clock: ToolLoopClock,
}

impl Default for ToolLoopOptions {
    fn default() -> Self {
        Self {
            tool_max_turns: None,
            permission_handler: None,
            error_formatter: std::sync::Arc::new(DefaultToolErrorFormatter),
            clock: ToolLoopClock::default(),
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
    let clock = options.clock.clone();
    let mut tool_turns = 0usize;
    let mut appended_messages = Vec::new();

    loop {
        let prompt = request.clone();
        let provider_response = stream_provider_turn(
            client,
            prompt,
            cancellation,
            &clock,
            &mut state,
            &mut on_progress,
        )
        .await?;
        state.capture_turn_response(&provider_response);
        let tool_calls = provider_response.tool_calls.clone();
        if !provider_response.finish_reason.is_tool_call() || tool_calls.is_empty() {
            append_provider_context_message(
                provider_response.message,
                &mut appended_messages,
                &mut on_progress,
            );
            return Ok(state.finish_at(clock.now(), appended_messages));
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
            let mut tool_call_context = ToolCallExecutionContext {
                executor: &executor,
                tool_definitions: &tool_definitions,
                cancellation,
                clock: &clock,
                permission_handler: options.permission_handler.as_ref(),
                error_formatter: &options.error_formatter,
                state: &mut state,
            };
            let execution = if cancellation.is_cancelled() {
                interrupted_tool_execution(&call)
            } else {
                execute_tool_call(&call, &mut tool_call_context, &mut on_progress).await
            };
            let update = runtime_tool_activity_update_from_result(
                &call,
                &execution.raw_result,
                execution.processed_error.as_ref(),
                &tool_definitions,
            );
            let visible_tool_output = runtime_tool_activity_update_token_text(&update);
            let suppress_counted_arguments =
                runtime_tool_activity_update_duplicates_tool_arguments(&update);
            let activity_id = update.activity_id.clone();
            on_progress(ToolLoopProgress::ToolActivityUpdated { update });
            state.observe_tool_activity_output(
                &activity_id,
                visible_tool_output.as_deref(),
                suppress_counted_arguments,
                clock.now(),
                &mut on_progress,
            );
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
            state.observe_tool_result_input(provider_result_content, clock.now(), &mut on_progress);
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
            return Ok(state.finish_at(clock.now(), appended_messages));
        }
    }
}

async fn stream_provider_turn<C, F>(
    client: &C,
    request: PromptRequest,
    cancellation: &CancellationToken,
    clock: &ToolLoopClock,
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
    state.mark_request_started(clock.now());
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
            state.observe_response_tool_calls_completed(&response, clock.now(), on_progress);
            Ok(response)
        }
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
    clock: &'a ToolLoopClock,
    provider_response: &'a mut Option<provider_protocol::PromptResponse>,
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
                    self.state.final_output_tokens = Some(output_tokens as usize);
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
    context: &mut ToolCallExecutionContext<'_>,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) -> ToolExecution {
    let runtime_call = tool_runtime::ToolCall::new(
        call.call_id.clone(),
        call.name.clone(),
        call.arguments.clone(),
    );

    let authorization = authorize_tool_call(&runtime_call, context).await;
    let raw_result = match authorization.denial_message {
        Some(message) => ToolResult::error(call.call_id.clone(), message),
        None => {
            execute_tool_with_progress(
                runtime_call,
                authorization.permission_snapshot,
                context,
                on_progress,
            )
            .await
        }
    };

    let processed_error = (raw_result.is_error
        && !is_command_execution_error(
            &raw_result,
            context.tool_definitions.definition(&call.name),
        ))
    .then(|| {
        context
            .error_formatter
            .format_tool_error(&call.name, &raw_result.content)
    });
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

fn is_command_execution_error(
    result: &ToolResult,
    definition: Option<&tool_runtime::ToolDefinition>,
) -> bool {
    if definition.map(|definition| definition.kind) != Some(ToolKind::Execute) {
        return false;
    }

    let Some(details) = result.details.as_ref() else {
        return false;
    };
    details
        .get("execution_kind")
        .and_then(serde_json::Value::as_str)
        == Some("command")
}

async fn execute_tool_with_progress(
    call: tool_runtime::ToolCall,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
    context: &mut ToolCallExecutionContext<'_>,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) -> ToolResult {
    let (progress_sender, mut progress_receiver) = tokio::sync::mpsc::unbounded_channel();
    let tool_context = ToolExecutionContext::new(context.cancellation)
        .with_permission_snapshot(permission_snapshot)
        .with_permission_handler(context.permission_handler.cloned())
        .with_progress_sink(ToolProgressSink::from_sender(progress_sender));
    let execution = context
        .executor
        .execute_tool_with_context(call, tool_context);
    tokio::pin!(execution);
    let mut progress_closed = false;

    let result = loop {
        tokio::select! {
            biased;
            maybe_progress = progress_receiver.recv(), if !progress_closed => {
                if let Some(progress) = maybe_progress {
                    emit_tool_progress(progress, context.clock, context.state, on_progress);
                } else {
                    progress_closed = true;
                };
            }
            result = &mut execution => break result,
        }
    };

    while let Ok(progress) = progress_receiver.try_recv() {
        emit_tool_progress(progress, context.clock, context.state, on_progress);
    }

    result
}

fn emit_tool_progress(
    progress: ToolProgress,
    clock: &ToolLoopClock,
    state: &mut RuntimeTurnState,
    on_progress: &mut impl FnMut(ToolLoopProgress),
) {
    match progress {
        ToolProgress::SystemMessage { message } => {
            on_progress(ToolLoopProgress::SystemMessage { message });
        }
        ToolProgress::TerminalUpdated { snapshot } => {
            let snapshot = runtime_terminal_snapshot(snapshot);
            on_progress(ToolLoopProgress::TerminalUpdated {
                snapshot: snapshot.clone(),
            });
            state.observe_terminal_snapshot_output(&snapshot, clock.now(), on_progress);
        }
        ToolProgress::ManagedSearchToolAuthorization { tool_name } => {
            if let Some(tool) = ManagedSearchTool::from_binary_name(&tool_name) {
                on_progress(ToolLoopProgress::ManagedSearchToolAuthorization { tool });
            }
        }
    }
}

fn runtime_terminal_snapshot(snapshot: ToolTerminalSnapshot) -> RuntimeTerminalSnapshot {
    RuntimeTerminalSnapshot {
        terminal_id: snapshot.terminal_id,
        command: snapshot.command,
        cwd: snapshot.cwd,
        output: snapshot.output,
        truncated: snapshot.truncated,
        exit_status: snapshot.exit_status.map(runtime_terminal_exit_status),
        released: snapshot.released,
    }
}

fn runtime_terminal_exit_status(status: ToolTerminalExitStatus) -> RuntimeTerminalExitStatus {
    RuntimeTerminalExitStatus {
        exit_code: status.exit_code,
        signal: status.signal,
    }
}

async fn authorize_tool_call(
    call: &tool_runtime::ToolCall,
    context: &mut ToolCallExecutionContext<'_>,
) -> ToolAuthorization {
    let Some(definition) = context.tool_definitions.definition(&call.name).cloned() else {
        return ToolAuthorization::allow(None);
    };

    match definition.permission_policy {
        ToolPermissionPolicy::Always => ToolAuthorization::allow(None),
        ToolPermissionPolicy::Never => ToolAuthorization::deny(format!(
            "{TOOL_PERMISSION_DENIED}: {} is not allowed",
            definition.name
        )),
        ToolPermissionPolicy::Ask => {
            let Some(permission_handler) = context.permission_handler else {
                return ToolAuthorization::deny(format!(
                    "{TOOL_PERMISSION_DENIED}: {} requires approval",
                    definition.name
                ));
            };
            let mut permission_request = ToolPermissionRequest::new(call.clone(), definition);
            let preview =
                permission_preview_from_executor(context.executor, call, context.cancellation)
                    .await;
            let permission_snapshot = preview
                .as_ref()
                .and_then(|preview| preview.snapshot.clone());
            if let Some(preview) = preview {
                permission_request = permission_request.with_preview(preview);
            }
            let permission_started_at = context.clock.now();
            match permission_handler
                .request_permission(permission_request, context.cancellation)
                .await
            {
                ToolPermissionDecision::Allow => {
                    context.state.record_permission_wait(
                        context
                            .clock
                            .now()
                            .saturating_duration_since(permission_started_at),
                    );
                    ToolAuthorization::allow(permission_snapshot)
                }
                ToolPermissionDecision::Deny { message } => {
                    context.state.record_permission_wait(
                        context
                            .clock
                            .now()
                            .saturating_duration_since(permission_started_at),
                    );
                    ToolAuthorization::deny(message)
                }
            }
        }
    }
}

async fn permission_preview_from_executor(
    executor: &ToolExecutorRegistry,
    call: &tool_runtime::ToolCall,
    cancellation: &CancellationToken,
) -> Option<ToolPermissionPreview> {
    if cancellation.is_cancelled() {
        return None;
    }
    let executor = executor.clone();
    let call = call.clone();
    let cancellation = cancellation.clone();
    tokio::task::spawn_blocking(move || executor.permission_preview(&call, &cancellation))
        .await
        .ok()
        .flatten()
}

struct ToolAuthorization {
    denial_message: Option<String>,
    permission_snapshot: Option<ToolPermissionFileSnapshot>,
}

impl ToolAuthorization {
    fn allow(permission_snapshot: Option<ToolPermissionFileSnapshot>) -> Self {
        Self {
            denial_message: None,
            permission_snapshot,
        }
    }

    fn deny(message: String) -> Self {
        Self {
            denial_message: Some(message),
            permission_snapshot: None,
        }
    }
}

struct ToolCallExecutionContext<'a> {
    executor: &'a ToolExecutorRegistry,
    tool_definitions: &'a ToolRegistry,
    cancellation: &'a CancellationToken,
    clock: &'a ToolLoopClock,
    permission_handler: Option<&'a SharedToolPermissionHandler>,
    error_formatter: &'a SharedToolErrorFormatter,
    state: &'a mut RuntimeTurnState,
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
    permission_wait_duration: Duration,
    final_output_tokens: Option<usize>,
    saw_tool_call_turn: bool,
    terminal_output_by_id: HashMap<String, String>,
    tool_call_ids_by_index: HashMap<usize, String>,
    tool_call_argument_output_by_index: HashMap<usize, String>,
    tool_call_argument_output_by_id: HashMap<String, String>,
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
            permission_wait_duration: Duration::ZERO,
            final_output_tokens: None,
            saw_tool_call_turn: false,
            terminal_output_by_id: HashMap::new(),
            tool_call_ids_by_index: HashMap::new(),
            tool_call_argument_output_by_index: HashMap::new(),
            tool_call_argument_output_by_id: HashMap::new(),
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

    fn record_permission_wait(&mut self, duration: Duration) {
        self.permission_wait_duration = self.permission_wait_duration.saturating_add(duration);
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
        self.mark_generated_output_started(now, on_progress);
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

    fn observe_tool_call_started(&mut self, index: usize, call_id: String) {
        self.tool_call_ids_by_index.insert(index, call_id.clone());
        if let Some(output) = self.tool_call_argument_output_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call_id, output.clone());
        }
    }

    fn observe_tool_call_arguments_delta(
        &mut self,
        index: usize,
        delta: &str,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        if delta.is_empty() {
            return;
        }
        self.mark_generated_output_started(now, on_progress);
        let output = self
            .tool_call_argument_output_by_index
            .entry(index)
            .or_default();
        output.push_str(delta);
        if let Some(call_id) = self.tool_call_ids_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call_id.clone(), output.clone());
        }
        self.observe_token_delta(delta, now, on_progress);
    }

    fn observe_response_tool_calls_completed(
        &mut self,
        response: &provider_protocol::PromptResponse,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        for (index, call) in response.tool_calls.iter().enumerate() {
            self.observe_tool_call_completed(index, call, now, on_progress);
        }
    }

    fn observe_tool_call_completed(
        &mut self,
        index: usize,
        call: &AiToolCall,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.tool_call_ids_by_index
            .insert(index, call.call_id.clone());
        if let Some(output) = self.tool_call_argument_output_by_id.get(&call.call_id) {
            self.tool_call_argument_output_by_index
                .entry(index)
                .or_insert_with(|| output.clone());
            return;
        }
        if let Some(output) = self.tool_call_argument_output_by_index.get(&index) {
            self.tool_call_argument_output_by_id
                .insert(call.call_id.clone(), output.clone());
            return;
        }

        let output = call.arguments.to_string();
        if output.is_empty() {
            return;
        }
        self.mark_generated_output_started(now, on_progress);
        self.tool_call_argument_output_by_index
            .insert(index, output.clone());
        self.tool_call_argument_output_by_id
            .insert(call.call_id.clone(), output.clone());
        self.observe_token_delta(&output, now, on_progress);
    }

    fn mark_generated_output_started(
        &mut self,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        self.first_token_at.get_or_insert(now);
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(ToolLoopProgress::Thinking { is_thinking: false });
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

    fn observe_tool_activity_output(
        &mut self,
        activity_id: &str,
        content: Option<&str>,
        suppress_counted_arguments: bool,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let Some(content) = content else {
            return;
        };
        if suppress_counted_arguments
            && self
                .tool_call_argument_output_by_id
                .contains_key(activity_id)
        {
            return;
        }
        let token_text = self.visible_tool_output_delta(activity_id, content);
        let token_text = token_text.as_deref().unwrap_or(content);
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.output_progress, token_text, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::OutputTokens { total_tokens });
    }

    fn observe_terminal_snapshot_output(
        &mut self,
        snapshot: &RuntimeTerminalSnapshot,
        now: Instant,
        on_progress: &mut impl FnMut(ToolLoopProgress),
    ) {
        let token_text = self.visible_tool_output_delta(&snapshot.terminal_id, &snapshot.output);
        self.terminal_output_by_id
            .insert(snapshot.terminal_id.clone(), snapshot.output.clone());
        let token_text = token_text.as_deref().unwrap_or(&snapshot.output);
        let Some(total_tokens) =
            observe_complete_token_total(&mut self.output_progress, token_text, now)
        else {
            return;
        };

        on_progress(ToolLoopProgress::OutputTokens { total_tokens });
    }

    fn visible_tool_output_delta(&self, activity_id: &str, content: &str) -> Option<String> {
        self.terminal_output_by_id
            .get(activity_id)
            .map(|previous| terminal_output_delta(previous, content).to_string())
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
            duration: finished_at
                .saturating_duration_since(request_started_at)
                .saturating_sub(self.permission_wait_duration),
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

fn runtime_tool_activity_update_token_text(update: &RuntimeToolActivityUpdate) -> Option<String> {
    if let Some(content) = update.content.as_ref() {
        let text = content
            .iter()
            .filter_map(runtime_tool_activity_content_token_text)
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            return Some(text);
        }
    }

    update.raw_output.as_ref().and_then(|raw| raw.token_text())
}

fn runtime_tool_activity_update_duplicates_tool_arguments(
    update: &RuntimeToolActivityUpdate,
) -> bool {
    update.content.as_ref().is_some_and(|content| {
        content
            .iter()
            .any(|content| matches!(content, RuntimeToolActivityContent::Diff { .. }))
    })
}

fn runtime_tool_activity_content_token_text(
    content: &RuntimeToolActivityContent,
) -> Option<String> {
    match content {
        RuntimeToolActivityContent::Text(text) | RuntimeToolActivityContent::Unknown(text) => {
            non_empty_token_text(text)
        }
        RuntimeToolActivityContent::Image { mime_type, uri } => {
            let text = uri
                .as_deref()
                .map(|uri| format!("image {mime_type} {uri}"))
                .unwrap_or_else(|| format!("image {mime_type}"));
            non_empty_token_text(&text)
        }
        RuntimeToolActivityContent::Audio { mime_type } => {
            non_empty_token_text(&format!("audio {mime_type}"))
        }
        RuntimeToolActivityContent::ResourceLink { uri, name, title } => {
            let mut text = format!("{name} {uri}");
            if let Some(title) = title.as_deref() {
                text.push('\n');
                text.push_str(title);
            }
            non_empty_token_text(&text)
        }
        RuntimeToolActivityContent::Resource {
            uri,
            mime_type,
            text,
        } => {
            let mut parts = vec![uri.clone()];
            if let Some(mime_type) = mime_type.as_deref() {
                parts.push(mime_type.to_string());
            }
            if let Some(text) = text.as_deref() {
                parts.push(text.to_string());
            }
            non_empty_token_text(&parts.join("\n"))
        }
        RuntimeToolActivityContent::Diff {
            path,
            old_text,
            new_text,
            ..
        } => {
            let mut parts = vec![path.clone()];
            if let Some(old_text) = old_text.as_deref() {
                parts.push(old_text.to_string());
            }
            parts.push(new_text.clone());
            non_empty_token_text(&parts.join("\n"))
        }
        RuntimeToolActivityContent::Terminal { .. } => None,
    }
}

fn non_empty_token_text(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

fn terminal_output_delta<'a>(previous: &str, current: &'a str) -> &'a str {
    if previous.is_empty() {
        return current;
    }
    if let Some(delta) = current.strip_prefix(previous) {
        return delta;
    }
    if previous.ends_with(current) {
        return "";
    }

    let mut overlap_len = 0usize;
    for (index, _) in current.char_indices().skip(1) {
        if previous.ends_with(&current[..index]) {
            overlap_len = index;
        }
    }

    &current[overlap_len..]
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::{Duration, Instant};

    use provider_protocol::{
        Message, MessageRole, ModelDescriptor, PromptRequest, PromptResponse, ProviderCapabilities,
        ProviderClient, ProviderError, ProviderFuture, StreamEvent, StreamEventSink, TokenUsage,
        ToolCall,
    };
    use runtime_domain::token_count::StreamingTokenProgress;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::{
        Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionContext,
        ToolExecutionFuture, ToolExecutorRegistry, ToolKind, ToolPermissionDecision,
        ToolPermissionFuture, ToolPermissionHandler, ToolPermissionPolicy, ToolPermissionPreview,
        ToolPermissionRequest, ToolProgress, ToolResult, ToolTerminalSnapshot,
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

    struct WriteArgumentStreamingProvider {
        calls: Mutex<usize>,
    }

    struct WriteArgumentCompletedProvider {
        calls: Mutex<usize>,
    }

    struct WriteArgumentResponseOnlyProvider {
        calls: Mutex<usize>,
    }

    fn write_arguments_for_token_tests() -> serde_json::Value {
        serde_json::json!({
            "path": "temp.md",
            "content": "generated write content ".repeat(80),
        })
    }

    impl ProviderClient for WriteArgumentStreamingProvider {
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
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::MessageStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    return Ok(response);
                }

                let arguments = write_arguments_for_token_tests();
                let content = arguments
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .expect("write test arguments should contain content")
                    .to_string();
                let call = ToolCall::new("call-write", "write", arguments);
                sink.emit(StreamEvent::ToolCallStarted {
                    index: 0,
                    call_id: "call-write".to_string(),
                    name: "write".to_string(),
                });
                sink.emit(StreamEvent::ToolCallArgumentsDelta {
                    index: 0,
                    delta: r#"{"path":"temp.md","content":""#.to_string(),
                });
                sink.emit(StreamEvent::ToolCallArgumentsDelta {
                    index: 0,
                    delta: content,
                });
                sink.emit(StreamEvent::ToolCallArgumentsDelta {
                    index: 0,
                    delta: r#""}"#.to_string(),
                });
                let response = PromptResponse::new(
                    Message::assistant_with_tool_calls(String::new(), vec![call.clone()]),
                    provider_protocol::FinishReason::ToolCalls,
                    None,
                    vec![call],
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

    impl ProviderClient for WriteArgumentCompletedProvider {
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
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::MessageStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    return Ok(response);
                }

                let call = ToolCall::new("call-write", "write", write_arguments_for_token_tests());
                sink.emit(StreamEvent::ToolCallStarted {
                    index: 0,
                    call_id: "call-write".to_string(),
                    name: "write".to_string(),
                });
                sink.emit(StreamEvent::ToolCallCompleted {
                    index: 0,
                    call: call.clone(),
                });
                let response = PromptResponse::new(
                    Message::assistant_with_tool_calls(String::new(), vec![call.clone()]),
                    provider_protocol::FinishReason::ToolCalls,
                    None,
                    vec![call],
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

    impl ProviderClient for WriteArgumentResponseOnlyProvider {
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
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::MessageStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = PromptResponse::new(
                        Message::text(MessageRole::Assistant, "done"),
                        provider_protocol::FinishReason::Stop,
                        None,
                        Vec::new(),
                    );
                    sink.emit(StreamEvent::MessageCompleted(response.clone()));
                    return Ok(response);
                }

                let call = ToolCall::new("call-write", "write", write_arguments_for_token_tests());
                let response = PromptResponse::new(
                    Message::assistant_with_tool_calls(String::new(), vec![call.clone()]),
                    provider_protocol::FinishReason::ToolCalls,
                    None,
                    vec![call],
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

    struct WriteLikeTool;

    impl Tool for WriteLikeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("write")
                .with_label("Write")
                .with_kind(ToolKind::Write)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move {
                let new_text = call
                    .arguments
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("written")
                    .to_string();
                let mut result = ToolResult::success(call.call_id, "written");
                result.details = Some(serde_json::json!({
                    "path": "temp.md",
                    "new_text": new_text,
                }));
                result
            })
        }
    }

    struct AskWriteLikeTool;

    impl Tool for AskWriteLikeTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("write")
                .with_label("Write")
                .with_kind(ToolKind::Write)
                .with_permission_policy(ToolPermissionPolicy::Ask)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move {
                let new_text = call
                    .arguments
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("written")
                    .to_string();
                ToolResult::success(call.call_id, "written").with_details(serde_json::json!({
                    "path": "temp.md",
                    "new_text": new_text,
                }))
            })
        }

        fn permission_preview(
            &self,
            call: &RuntimeToolCall,
            _cancellation: &CancellationToken,
        ) -> Option<ToolPermissionPreview> {
            Some(ToolPermissionPreview {
                path: "temp.md".to_string(),
                old_text: None,
                new_text: call
                    .arguments
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                is_truncated: false,
                snapshot: None,
            })
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

    struct LargeOutputTool;

    impl Tool for LargeOutputTool {
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
            Box::pin(async move { ToolResult::success(call.call_id, "tool output ".repeat(80)) })
        }
    }

    struct AskEchoTool;

    impl Tool for AskEchoTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Other)
                .with_permission_policy(ToolPermissionPolicy::Ask)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
        }
    }

    struct SleepyAllowPermissionHandler;

    impl ToolPermissionHandler for SleepyAllowPermissionHandler {
        fn request_permission<'a>(
            &'a self,
            _request: ToolPermissionRequest,
            _cancellation: &'a CancellationToken,
        ) -> ToolPermissionFuture<'a> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                ToolPermissionDecision::Allow
            })
        }
    }

    struct CapturingAllowPermissionHandler {
        preview: Arc<Mutex<Option<ToolPermissionPreview>>>,
    }

    impl ToolPermissionHandler for CapturingAllowPermissionHandler {
        fn request_permission<'a>(
            &'a self,
            request: ToolPermissionRequest,
            _cancellation: &'a CancellationToken,
        ) -> ToolPermissionFuture<'a> {
            *self.preview.lock().expect("preview lock should not poison") = request.preview;
            Box::pin(async { ToolPermissionDecision::Allow })
        }
    }

    struct BlockingPreviewProbePermissionHandler {
        preview: Arc<Mutex<Option<ToolPermissionPreview>>>,
        timer_fired: Arc<AtomicBool>,
        timer_fired_before_permission: Arc<AtomicBool>,
    }

    impl ToolPermissionHandler for BlockingPreviewProbePermissionHandler {
        fn request_permission<'a>(
            &'a self,
            request: ToolPermissionRequest,
            _cancellation: &'a CancellationToken,
        ) -> ToolPermissionFuture<'a> {
            *self.preview.lock().expect("preview lock should not poison") = request.preview;
            self.timer_fired_before_permission
                .store(self.timer_fired.load(Ordering::SeqCst), Ordering::SeqCst);
            Box::pin(async { ToolPermissionDecision::Allow })
        }
    }

    struct PermissionEventCountProbe {
        events: Arc<Mutex<Vec<ToolLoopProgress>>>,
        event_count_at_permission: Arc<Mutex<Option<usize>>>,
    }

    impl ToolPermissionHandler for PermissionEventCountProbe {
        fn request_permission<'a>(
            &'a self,
            _request: ToolPermissionRequest,
            _cancellation: &'a CancellationToken,
        ) -> ToolPermissionFuture<'a> {
            let event_count = self
                .events
                .lock()
                .expect("events lock should not poison")
                .len();
            *self
                .event_count_at_permission
                .lock()
                .expect("event count lock should not poison") = Some(event_count);
            Box::pin(async { ToolPermissionDecision::Allow })
        }
    }

    struct AskPreviewTool;

    impl Tool for AskPreviewTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Edit)
                .with_permission_policy(ToolPermissionPolicy::Ask)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
        }

        fn permission_preview(
            &self,
            _call: &RuntimeToolCall,
            _cancellation: &CancellationToken,
        ) -> Option<ToolPermissionPreview> {
            Some(ToolPermissionPreview {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
                snapshot: None,
            })
        }
    }

    struct SlowPreviewTool;

    impl Tool for SlowPreviewTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Echo")
                .with_kind(ToolKind::Edit)
                .with_permission_policy(ToolPermissionPolicy::Ask)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
        }

        fn permission_preview(
            &self,
            _call: &RuntimeToolCall,
            _cancellation: &CancellationToken,
        ) -> Option<ToolPermissionPreview> {
            std::thread::sleep(Duration::from_millis(500));
            Some(ToolPermissionPreview {
                path: "temp.md".to_string(),
                old_text: Some("old\n".to_string()),
                new_text: "new\n".to_string(),
                is_truncated: false,
                snapshot: None,
            })
        }
    }

    struct FailingExecuteTool;

    impl Tool for FailingExecuteTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo")
                .with_label("Shell:")
                .with_kind(ToolKind::Execute)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move {
                let mut result =
                    ToolResult::error(call.call_id, "before failure\n\nCommand exited with code 7");
                result.details = Some(serde_json::json!({
                    "execution_kind": "command",
                    "exit_code": 7,
                    "duration_ms": 250,
                    "timed_out": false,
                    "cancelled": false
                }));
                result.display_content = Some("before failure".to_string());
                result
            })
        }
    }

    struct TerminalProgressTool;

    impl Tool for TerminalProgressTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("run")
                .with_label("Run")
                .with_kind(ToolKind::Execute)
                .with_permission_policy(ToolPermissionPolicy::Always)
        }

        fn execute<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "done") })
        }

        fn execute_with_context<'a>(
            &'a self,
            call: RuntimeToolCall,
            context: ToolExecutionContext<'a>,
        ) -> ToolExecutionFuture<'a> {
            context.emit(ToolProgress::TerminalUpdated {
                snapshot: ToolTerminalSnapshot {
                    terminal_id: call.call_id.clone(),
                    command: Some("cargo check".to_string()),
                    cwd: Some("/workspace".to_string()),
                    output: "Checking lumos".to_string(),
                    truncated: false,
                    exit_status: None,
                    released: false,
                },
            });
            Box::pin(async move { ToolResult::success(call.call_id, "done") })
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
    async fn execute_tool_command_errors_preserve_raw_output_and_details() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(FailingExecuteTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let mut events = Vec::new();

        let completion = run_tool_loop(
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
        .await
        .expect("runtime should complete");

        let tool_result = completion.appended_messages[1]
            .first_tool_result()
            .expect("tool result should be appended");
        assert!(tool_result.is_error);
        assert_eq!(
            tool_result.content,
            "before failure\n\nCommand exited with code 7"
        );
        assert_eq!(
            tool_result
                .details
                .as_ref()
                .and_then(|details| details.get("exit_code"))
                .and_then(serde_json::Value::as_i64),
            Some(7)
        );

        let raw_output = events.iter().find_map(|event| match event {
            ToolLoopProgress::ToolActivityUpdated { update } if update.activity_id == "call-1" => {
                update.raw_output.as_ref()
            }
            _ => None,
        });
        let raw_output = raw_output.expect("command failure should preserve raw output");
        assert_eq!(raw_output.display_text().as_deref(), Some("before failure"));
        assert_eq!(
            raw_output
                .tool_result_details()
                .and_then(|details| details.get("duration_ms"))
                .and_then(serde_json::Value::as_u64),
            Some(250)
        );
    }

    #[tokio::test]
    async fn runtime_forwards_terminal_progress_from_tool_execution() {
        struct RunOnceProvider {
            calls: Mutex<usize>,
        }

        impl ProviderClient for RunOnceProvider {
            fn stream_prompt<'a>(
                &'a self,
                request: PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptResponse, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    if request
                        .messages
                        .iter()
                        .any(|message| message.first_tool_result().is_some())
                    {
                        let response = PromptResponse::new(
                            Message::text(MessageRole::Assistant, "done"),
                            provider_protocol::FinishReason::Stop,
                            None,
                            Vec::new(),
                        );
                        sink.emit(StreamEvent::MessageCompleted(response.clone()));
                        return Ok(response);
                    }

                    let call = ToolCall::new(
                        "call-run",
                        "run",
                        serde_json::json!({ "command": "cargo check" }),
                    );
                    let response = PromptResponse::new(
                        Message::assistant_with_tool_calls(String::new(), vec![call.clone()]),
                        provider_protocol::FinishReason::ToolCalls,
                        None,
                        vec![call],
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

        let provider = RunOnceProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(TerminalProgressTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "run cargo check")],
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
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ToolLoopProgress::ToolActivityStarted { activity }
                    if activity.content.iter().any(|content| {
                        matches!(
                            content,
                            runtime_domain::session::RuntimeToolActivityContent::Terminal { terminal_id }
                                if terminal_id == "call-run"
                        )
                    })
            )
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ToolLoopProgress::TerminalUpdated { snapshot }
                    if snapshot.terminal_id == "call-run"
                        && snapshot.output == "Checking lumos"
                        && snapshot.command.as_deref() == Some("cargo check")
            )
        }));
        let terminal_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::TerminalUpdated { .. }))
            .expect("terminal update should be emitted");
        let tool_update_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ToolActivityUpdated { .. }))
            .expect("tool activity update should be emitted");
        assert!(
            events[terminal_index + 1..tool_update_index]
                .iter()
                .any(|event| matches!(event, ToolLoopProgress::OutputTokens { total_tokens } if *total_tokens > 0)),
            "streaming terminal output should update visible output tokens before the final tool update: {events:?}"
        );
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
    async fn tool_activity_update_output_emits_token_progress_before_provider_input() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(LargeOutputTool);
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

        let update_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ToolActivityUpdated { .. }))
            .expect("tool activity update should be emitted");
        let input_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::InputTokens { .. }))
            .expect("tool result input tokens should still be emitted later");
        assert!(
            update_index < input_index,
            "tool update should render before provider-context input accounting: {events:?}"
        );
        let previous_output_tokens = events[..update_index]
            .iter()
            .rev()
            .filter_map(|event| match event {
                ToolLoopProgress::OutputTokens { total_tokens } => Some(*total_tokens),
                _ => None,
            })
            .next()
            .unwrap_or_default();
        let tool_output_tokens = events[update_index + 1..input_index]
            .iter()
            .find_map(|event| match event {
                ToolLoopProgress::OutputTokens { total_tokens } => Some(*total_tokens),
                _ => None,
            })
            .expect("visible tool activity output should update output token progress");

        assert!(
            tool_output_tokens > previous_output_tokens,
            "tool output should increase visible output tokens, previous={previous_output_tokens}, next={tool_output_tokens}"
        );
    }

    #[tokio::test]
    async fn streaming_tool_call_arguments_emit_output_tokens_before_tool_execution() {
        let provider = WriteArgumentStreamingProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(WriteLikeTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "write a file")],
        );
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

        let first_provider_context_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ProviderContextMessage { .. }))
            .expect("assistant tool-call message should be committed");
        let token_progress_before_tool_execution = events[..first_provider_context_index]
            .iter()
            .rev()
            .filter_map(|event| match event {
                ToolLoopProgress::OutputTokens { total_tokens } => Some(*total_tokens),
                _ => None,
            })
            .next()
            .unwrap_or_default();

        assert!(
            token_progress_before_tool_execution > 0,
            "streamed tool-call arguments should count as assistant output before tool execution: {events:?}"
        );
    }

    #[tokio::test]
    async fn ask_write_streamed_arguments_count_before_permission_and_skip_final_diff() {
        let provider = WriteArgumentStreamingProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(AskWriteLikeTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "write a file")],
        );
        let cancellation = CancellationToken::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let event_count_at_permission = Arc::new(Mutex::new(None));

        let _ = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                permission_handler: Some(Arc::new(PermissionEventCountProbe {
                    events: Arc::clone(&events),
                    event_count_at_permission: Arc::clone(&event_count_at_permission),
                })),
                ..ToolLoopOptions::default()
            },
            |event| {
                events
                    .lock()
                    .expect("events lock should not poison")
                    .push(event)
            },
        )
        .await
        .expect("runtime should complete");

        let events = events.lock().expect("events lock should not poison");
        let event_count_at_permission = event_count_at_permission
            .lock()
            .expect("event count lock should not poison")
            .expect("permission handler should be invoked");
        assert!(
            events[..event_count_at_permission]
                .iter()
                .any(|event| matches!(event, ToolLoopProgress::OutputTokens { total_tokens } if *total_tokens > 0)),
            "streamed write arguments should emit output token progress before permission approval: {events:?}"
        );

        let update_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ToolActivityUpdated { .. }))
            .expect("write tool update should be emitted");
        let input_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::InputTokens { .. }))
            .expect("tool result input tokens should be emitted");
        let output_after_update = events[update_index + 1..input_index]
            .iter()
            .any(|event| matches!(event, ToolLoopProgress::OutputTokens { .. }));
        assert!(
            !output_after_update,
            "final write diff should not be counted after approval when streamed arguments were already counted: {events:?}"
        );
    }

    #[tokio::test]
    async fn response_only_write_arguments_count_before_permission_and_skip_final_diff() {
        let provider = WriteArgumentResponseOnlyProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(AskWriteLikeTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "write a file")],
        );
        let cancellation = CancellationToken::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let event_count_at_permission = Arc::new(Mutex::new(None));

        let _ = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                permission_handler: Some(Arc::new(PermissionEventCountProbe {
                    events: Arc::clone(&events),
                    event_count_at_permission: Arc::clone(&event_count_at_permission),
                })),
                ..ToolLoopOptions::default()
            },
            |event| {
                events
                    .lock()
                    .expect("events lock should not poison")
                    .push(event)
            },
        )
        .await
        .expect("runtime should complete");

        let events = events.lock().expect("events lock should not poison");
        let event_count_at_permission = event_count_at_permission
            .lock()
            .expect("event count lock should not poison")
            .expect("permission handler should be invoked");
        assert!(
            events[..event_count_at_permission]
                .iter()
                .any(|event| matches!(event, ToolLoopProgress::OutputTokens { total_tokens } if *total_tokens > 0)),
            "completed write arguments should be counted before permission even if the provider omitted argument stream events: {events:?}"
        );

        let update_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ToolActivityUpdated { .. }))
            .expect("write tool update should be emitted");
        let input_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::InputTokens { .. }))
            .expect("tool result input tokens should be emitted");
        let output_after_update = events[update_index + 1..input_index]
            .iter()
            .any(|event| matches!(event, ToolLoopProgress::OutputTokens { .. }));
        assert!(
            !output_after_update,
            "final write diff should not be counted after approval when response arguments were already counted: {events:?}"
        );
    }

    #[tokio::test]
    async fn completed_tool_call_arguments_count_before_write_execution() {
        let provider = WriteArgumentCompletedProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(WriteLikeTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![Message::text(MessageRole::User, "write a file")],
        );
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

        let first_provider_context_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ProviderContextMessage { .. }))
            .expect("assistant tool-call message should be committed");
        let token_progress_before_tool_execution = events[..first_provider_context_index]
            .iter()
            .rev()
            .filter_map(|event| match event {
                ToolLoopProgress::OutputTokens { total_tokens } => Some(*total_tokens),
                _ => None,
            })
            .next()
            .unwrap_or_default();

        assert!(
            token_progress_before_tool_execution > 0,
            "completed tool-call arguments should count as assistant output before tool execution: {events:?}"
        );

        let update_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::ToolActivityUpdated { .. }))
            .expect("write tool update should be emitted");
        let input_index = events
            .iter()
            .position(|event| matches!(event, ToolLoopProgress::InputTokens { .. }))
            .expect("tool result input tokens should be emitted");
        let output_after_update = events[update_index + 1..input_index]
            .iter()
            .any(|event| matches!(event, ToolLoopProgress::OutputTokens { .. }));
        assert!(
            !output_after_update,
            "write diff should not be counted again after completed arguments were counted: {events:?}"
        );
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
        let _ = progress.observe_delta(
            &serde_json::json!({ "text": "hi" }).to_string(),
            started_at + Duration::from_millis(50),
        );
        let _ = super::observe_complete_token_total(
            &mut progress,
            "echoed",
            started_at + Duration::from_millis(100),
        );
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

    #[tokio::test]
    async fn tool_loop_metrics_exclude_permission_wait_time() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(AskEchoTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let wall_start = Instant::now();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                permission_handler: Some(std::sync::Arc::new(SleepyAllowPermissionHandler)),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let wall_elapsed = wall_start.elapsed();
        let metrics = completion.metrics.expect("metrics should be recorded");

        assert!(
            wall_elapsed >= Duration::from_millis(100),
            "test should actually wait for permission approval: {:?}",
            wall_elapsed
        );
        assert!(
            wall_elapsed.saturating_sub(metrics.duration) >= Duration::from_millis(90),
            "permission wait should be excluded from duration: wall={:?}, metrics={:?}",
            wall_elapsed,
            metrics.duration
        );
    }

    #[tokio::test]
    async fn tool_loop_passes_permission_preview_from_executor() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(AskPreviewTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let captured_preview = Arc::new(Mutex::new(None));

        run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                permission_handler: Some(std::sync::Arc::new(CapturingAllowPermissionHandler {
                    preview: Arc::clone(&captured_preview),
                })),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let preview = captured_preview
            .lock()
            .expect("preview lock should not poison")
            .clone()
            .expect("permission request should include executor preview");
        assert_eq!(preview.path, "temp.md");
        assert_eq!(preview.old_text.as_deref(), Some("old\n"));
        assert_eq!(preview.new_text, "new\n");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn permission_preview_uses_blocking_executor_on_current_thread_runtime() {
        let provider = FakeProvider {
            calls: Mutex::new(0),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(SlowPreviewTool);
        let request =
            PromptRequest::new("qwen3", vec![Message::text(MessageRole::User, "call echo")]);
        let cancellation = CancellationToken::new();
        let captured_preview = Arc::new(Mutex::new(None));
        let timer_fired = Arc::new(AtomicBool::new(false));
        let timer_fired_before_permission = Arc::new(AtomicBool::new(false));
        let timer_fired_for_task = Arc::clone(&timer_fired);

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            timer_fired_for_task.store(true, Ordering::SeqCst);
        });

        run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                permission_handler: Some(std::sync::Arc::new(
                    BlockingPreviewProbePermissionHandler {
                        preview: Arc::clone(&captured_preview),
                        timer_fired: Arc::clone(&timer_fired),
                        timer_fired_before_permission: Arc::clone(&timer_fired_before_permission),
                    },
                )),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
        .expect("runtime should complete");

        assert!(
            timer_fired_before_permission.load(Ordering::SeqCst),
            "permission preview should not block the current-thread runtime reactor"
        );
        assert!(
            captured_preview
                .lock()
                .expect("preview lock should not poison")
                .is_some(),
            "permission request should still include the generated preview"
        );
    }
}
