//! Tool loop runtime 的 provider streaming 与工具轮次编排入口。

use provider_protocol::{ContentBlock, ConversationItem, PromptRequest, ProviderClient};
use tokio_util::sync::CancellationToken;
use tool_runtime::ToolExecutorRegistry;

use crate::{
    activity::{runtime_tool_activity_from_call, runtime_tool_activity_update_from_result},
    error::ToolLoopError,
};

mod execution;
mod state;
mod streaming;
mod types;

use execution::{
    ToolCallExecutionContext, ai_tool_definitions_from_registry, execute_tool_call,
    interrupted_tool_execution,
};
use state::{
    RuntimeTurnState, runtime_tool_activity_update_duplicates_tool_arguments,
    runtime_tool_activity_update_token_text,
};
use streaming::{
    append_provider_context_item, append_provider_context_items, stream_provider_turn,
};

pub use types::{
    ToolLoopClock, ToolLoopCompletion, ToolLoopOptions, ToolLoopProgress, ToolLoopResponse,
};

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
    if request.items.is_empty() {
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
    let mut appended_items = Vec::new();

    loop {
        let provider_completion = stream_provider_turn(
            client,
            &request,
            cancellation,
            &clock,
            &mut state,
            &mut on_progress,
        )
        .await?;
        let tool_calls = extract_tool_calls(&provider_completion.items);
        if !provider_completion.finish_reason.is_tool_call() || tool_calls.is_empty() {
            append_provider_context_items(
                &provider_completion.items,
                &mut appended_items,
                &mut on_progress,
            );
            return Ok(state.finish_at(clock.now(), appended_items));
        }

        if let Some(max_turns) = options.tool_max_turns
            && tool_turns >= max_turns
        {
            return Err(ToolLoopError::ToolTurnLimit { max_turns });
        }
        tool_turns = tool_turns.saturating_add(1);
        request
            .items
            .extend(provider_completion.items.iter().cloned());
        append_provider_context_items(
            &provider_completion.items,
            &mut appended_items,
            &mut on_progress,
        );

        let mut tool_result_batch = Vec::new();
        for call in &tool_calls {
            let activity = runtime_tool_activity_from_call(call, &tool_definitions);
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
                interrupted_tool_execution(call)
            } else {
                execute_tool_call(call, &mut tool_call_context, &mut on_progress).await
            };
            let update = runtime_tool_activity_update_from_result(
                call,
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
            let tool_result_item = ConversationItem::tool_result(
                execution.provider_result.call_id.clone(),
                vec![ContentBlock::Text(
                    execution.provider_result.content.clone(),
                )],
                execution.provider_result.is_error,
            );
            tool_result_batch.push((tool_result_item, execution.raw_result.terminate));
        }

        let should_terminate_after_batch = tool_result_batch
            .iter()
            .all(|(_, should_terminate)| *should_terminate);
        for (tool_result_item, _) in tool_result_batch {
            if should_terminate_after_batch {
                append_provider_context_item(
                    tool_result_item,
                    &mut appended_items,
                    &mut on_progress,
                );
                continue;
            }
            let provider_result_content = tool_result_item.text_content();
            state.observe_tool_result_input(
                &provider_result_content,
                clock.now(),
                &mut on_progress,
            );
            request.items.push(tool_result_item.clone());
            append_provider_context_item(tool_result_item, &mut appended_items, &mut on_progress);
        }
        if cancellation.is_cancelled() {
            return Err(ToolLoopError::Cancelled);
        }
        if should_terminate_after_batch {
            return Ok(state.finish_at(clock.now(), appended_items));
        }
    }
}

fn extract_tool_calls(items: &[ConversationItem]) -> Vec<provider_protocol::ToolCall> {
    items
        .iter()
        .flat_map(|item| item.tool_calls().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    use provider_protocol::{
        ContentBlock, ConversationItem, ModelDescriptor, PromptCompletion, PromptRequest,
        ProviderCapabilities, ProviderClient, ProviderError, ProviderFuture, Role, StreamEvent,
        StreamEventSink, TokenUsage, ToolCall,
    };
    use runtime_domain::token_count::StreamingTokenProgress;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::{
        Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionContext,
        ToolExecutionFuture, ToolExecutorRegistry, ToolKind, ToolPermissionDecision,
        ToolPermissionFuture, ToolPermissionHandler, ToolPermissionPolicy, ToolPermissionPreview,
        ToolPermissionRequest, ToolProgress, ToolResult, ToolTerminalSnapshot,
    };

    use super::{ToolLoopClock, ToolLoopOptions, ToolLoopProgress, run_tool_loop};

    fn text_completion(role: Role, text: &str) -> PromptCompletion {
        PromptCompletion::new(
            vec![ConversationItem::text(role, text)],
            provider_protocol::FinishReason::Stop,
            None,
        )
    }

    fn text_completion_with_usage(role: Role, text: &str, usage: TokenUsage) -> PromptCompletion {
        PromptCompletion::new(
            vec![ConversationItem::text(role, text)],
            provider_protocol::FinishReason::Stop,
            Some(usage),
        )
    }

    fn tool_call_completion(text: String, calls: Vec<ToolCall>) -> PromptCompletion {
        PromptCompletion::new(
            vec![ConversationItem::assistant_with_tool_calls(text, calls)],
            provider_protocol::FinishReason::ToolCalls,
            None,
        )
    }

    fn tool_call_completion_with_usage(
        text: String,
        calls: Vec<ToolCall>,
        usage: TokenUsage,
    ) -> PromptCompletion {
        PromptCompletion::new(
            vec![ConversationItem::assistant_with_tool_calls(text, calls)],
            provider_protocol::FinishReason::ToolCalls,
            Some(usage),
        )
    }

    fn has_tool_result(items: &[ConversationItem]) -> bool {
        items
            .iter()
            .any(|item| matches!(item, ConversationItem::ToolResult { .. }))
    }

    struct FakeProvider {
        calls: Mutex<usize>,
    }

    impl ProviderClient for FakeProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
                if *calls == 1 {
                    let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                    sink.emit(StreamEvent::TextDelta("checking".to_string()));
                    let response = tool_call_completion("checking".to_string(), vec![call]);
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            _request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                sink.emit(StreamEvent::TurnStarted);
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
                let response = text_completion_with_usage(
                    Role::Assistant,
                    "done",
                    TokenUsage::new(None, Some(5), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            _request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(40)).await;
                sink.emit(StreamEvent::TurnStarted);
                tokio::time::sleep(Duration::from_millis(40)).await;
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                sink.emit(StreamEvent::TurnStarted);
                if !has_tool_result {
                    let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                    sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(20),
                        None,
                    )));
                    let response = tool_call_completion_with_usage(
                        "hello world".to_string(),
                        vec![call],
                        TokenUsage::new(None, Some(20), None),
                    );
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(5),
                        None,
                    )));
                    let response = text_completion_with_usage(
                        Role::Assistant,
                        "done",
                        TokenUsage::new(None, Some(5), None),
                    );
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    struct MixedUsageProvider;

    impl ProviderClient for MixedUsageProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                sink.emit(StreamEvent::TurnStarted);
                if !has_tool_result {
                    let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                    sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(20),
                        None,
                    )));
                    let response = tool_call_completion_with_usage(
                        "hello world".to_string(),
                        vec![call],
                        TokenUsage::new(None, Some(20), None),
                    );
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    #[derive(Clone)]
    struct ManualClock {
        started_at: Instant,
        elapsed_ms: Arc<AtomicU64>,
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                started_at: Instant::now(),
                elapsed_ms: Arc::new(AtomicU64::new(0)),
            }
        }

        fn now(&self) -> Instant {
            self.started_at + Duration::from_millis(self.elapsed_ms.load(Ordering::SeqCst))
        }

        fn advance(&self, duration: Duration) {
            let elapsed_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
            self.elapsed_ms.fetch_add(elapsed_ms, Ordering::SeqCst);
        }

        fn tool_loop_clock(&self) -> ToolLoopClock {
            let clock = self.clone();
            ToolLoopClock::new(move || clock.now())
        }
    }

    struct TimedToolLoopProvider {
        clock: ManualClock,
    }

    impl ProviderClient for TimedToolLoopProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                sink.emit(StreamEvent::TurnStarted);
                self.clock.advance(Duration::from_millis(10));
                if !has_tool_result {
                    let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                    sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                    self.clock.advance(Duration::from_millis(30));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(20),
                        None,
                    )));
                    let response = tool_call_completion_with_usage(
                        "hello world".to_string(),
                        vec![call],
                        TokenUsage::new(None, Some(20), None),
                    );
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    self.clock.advance(Duration::from_millis(30));
                    sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                        None,
                        Some(5),
                        None,
                    )));
                    let response = text_completion_with_usage(
                        Role::Assistant,
                        "done",
                        TokenUsage::new(None, Some(5), None),
                    );
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    struct ResponseOnlyTimedTextProvider {
        clock: ManualClock,
    }

    impl ProviderClient for ResponseOnlyTimedTextProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                sink.emit(StreamEvent::TurnStarted);
                self.clock.advance(Duration::from_millis(100));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    struct MultiToolBatchProvider {
        calls: Mutex<usize>,
        request_items: Mutex<Vec<Vec<ConversationItem>>>,
    }

    impl MultiToolBatchProvider {
        fn new() -> Self {
            Self {
                calls: Mutex::new(0),
                request_items: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProviderClient for MultiToolBatchProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                self.request_items
                    .lock()
                    .expect("request items lock should not poison")
                    .push(request.items.clone());
                let call_count = {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    *calls
                };
                sink.emit(StreamEvent::TurnStarted);
                if call_count == 1 {
                    let terminating_call = ToolCall::new(
                        "call-terminate",
                        "echo",
                        r#"{"terminate":true}"#.to_string(),
                    );
                    let continuing_call = ToolCall::new(
                        "call-continue",
                        "echo",
                        r#"{"terminate":false}"#.to_string(),
                    );
                    let calls = vec![terminating_call, continuing_call];
                    sink.emit(StreamEvent::TextDelta("checking".to_string()));
                    let response = tool_call_completion("checking".to_string(), calls);
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    Ok(response)
                } else {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    fn write_arguments_for_token_tests() -> String {
        serde_json::json!({
            "path": "temp.md",
            "content": "generated write content ".repeat(80),
        })
        .to_string()
    }

    impl ProviderClient for WriteArgumentStreamingProvider {
        fn stream_prompt<'a>(
            &'a self,
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::TurnStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    return Ok(response);
                }

                let arguments = write_arguments_for_token_tests();
                let arguments_value: serde_json::Value =
                    serde_json::from_str(&arguments).expect("valid JSON");
                let content = arguments_value
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
                let response = tool_call_completion(String::new(), vec![call]);
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::TurnStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
                let response = tool_call_completion(String::new(), vec![call]);
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            request: &'a PromptRequest,
            sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async move {
                let has_tool_result = has_tool_result(&request.items);
                {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                }
                sink.emit(StreamEvent::TurnStarted);
                if has_tool_result {
                    sink.emit(StreamEvent::TextDelta("done".to_string()));
                    let response = text_completion(Role::Assistant, "done");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
                    return Ok(response);
                }

                let call = ToolCall::new("call-write", "write", write_arguments_for_token_tests());
                let response = tool_call_completion(String::new(), vec![call]);
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
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

    struct ClockAdvanceTool {
        clock: ManualClock,
    }

    impl Tool for ClockAdvanceTool {
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
                self.clock.advance(Duration::from_millis(100));
                ToolResult::success(call.call_id, "echoed")
            })
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
                    output: "Checking hunea".to_string(),
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
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

        let final_item = completion
            .response
            .items
            .last()
            .expect("final assistant item should be preserved");
        assert_eq!(final_item.role(), Some(Role::Assistant));
        assert_eq!(final_item.text_content(), "done");
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        );
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

        let ConversationItem::ToolResult {
            content: tool_result_content,
            is_error: tool_result_is_error,
            ..
        } = &completion.response.items[1]
        else {
            panic!("tool result should be appended");
        };
        assert!(tool_result_is_error);
        assert_eq!(
            tool_result_content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            "before failure\n\nCommand exited with code 7"
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
                request: &'a PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    if has_tool_result(&request.items) {
                        let response = text_completion(Role::Assistant, "done");
                        sink.emit(StreamEvent::TurnCompleted(response.clone()));
                        return Ok(response);
                    }

                    let call = ToolCall::new(
                        "call-run",
                        "run",
                        r#"{"command":"cargo check"}"#.to_string(),
                    );
                    let response = tool_call_completion(String::new(), vec![call]);
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            vec![ConversationItem::text(Role::User, "run cargo check")],
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

        let final_item = completion
            .response
            .items
            .last()
            .expect("final assistant item should be preserved");
        assert_eq!(final_item.role(), Some(Role::Assistant));
        assert_eq!(final_item.text_content(), "done");
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
                        && snapshot.output == "Checking hunea"
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
    async fn provider_context_items_emit_before_follow_up_provider_failure() {
        struct FailingFollowUpProvider {
            calls: Mutex<usize>,
        }

        impl ProviderClient for FailingFollowUpProvider {
            fn stream_prompt<'a>(
                &'a self,
                _request: &'a PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    if *calls == 1 {
                        let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                        let response = tool_call_completion(String::new(), vec![call]);
                        sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
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
        .expect_err("follow-up provider error should fail the turn");

        assert!(matches!(
            error,
            super::ToolLoopError::Provider(ProviderError::Transport(_))
        ));
        let committed_kinds = events
            .iter()
            .filter_map(|event| match event {
                ToolLoopProgress::ProviderContextItem { item } => match item {
                    item if item.role() == Some(Role::Assistant) => Some("assistant"),
                    item if item.role() == Some(Role::User) => Some("user"),
                    ConversationItem::ToolResult { .. } => Some("tool_result"),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(committed_kinds, vec!["assistant", "tool_result"]);
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
                _request: &'a PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
                Box::pin(async move {
                    let mut calls = self.calls.lock().expect("fake lock should not poison");
                    *calls += 1;
                    let first = ToolCall::new("call-1", "cancel_once", "{}");
                    let second = ToolCall::new("call-2", "cancel_once", "{}");
                    let response = tool_call_completion(String::new(), vec![first, second]);
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
            vec![ConversationItem::text(Role::User, "call tools")],
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
        let committed_items = events
            .iter()
            .filter_map(|event| match event {
                ToolLoopProgress::ProviderContextItem { item } => Some(item),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(committed_items.len(), 3);
        assert_eq!(committed_items[0].role(), Some(Role::Assistant));
        assert!(matches!(
            committed_items[1],
            ConversationItem::ToolResult { .. }
        ));
        let ConversationItem::ToolResult {
            is_error: interrupted_is_error,
            content: interrupted_content,
            ..
        } = &committed_items[2]
        else {
            panic!("interrupted tool should have a synthetic result");
        };
        assert!(interrupted_is_error);
        assert_eq!(
            interrupted_content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(""),
            "Tool execution interrupted"
        );
    }

    #[tokio::test]
    async fn usage_updates_replace_output_token_metric() {
        let provider = UsageProvider;
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "count tokens")],
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
        let request =
            PromptRequest::new("qwen3", vec![ConversationItem::text(Role::User, "hello")]);
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
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
            vec![ConversationItem::text(Role::User, "write a file")],
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
            .position(|event| matches!(event, ToolLoopProgress::ProviderContextItem { .. }))
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
            vec![ConversationItem::text(Role::User, "write a file")],
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
            vec![ConversationItem::text(Role::User, "write a file")],
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
            vec![ConversationItem::text(Role::User, "write a file")],
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
            .position(|event| matches!(event, ToolLoopProgress::ProviderContextItem { .. }))
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
                _request: &'a PromptRequest,
                sink: &'a mut (dyn StreamEventSink + Send),
            ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
                Box::pin(async move {
                    sink.emit(StreamEvent::TurnStarted);
                    sink.emit(StreamEvent::TextDelta("hello".to_string()));
                    sink.emit(StreamEvent::TextDelta(" world".to_string()));
                    let response = text_completion(Role::Assistant, "hello world");
                    sink.emit(StreamEvent::TurnCompleted(response.clone()));
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
        let request =
            PromptRequest::new("gpt-4o", vec![ConversationItem::text(Role::User, "hello")]);
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
    async fn tool_loop_metrics_sum_provider_output_usage_without_tool_output() {
        let provider = ToolLoopUsageProvider;
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request = PromptRequest::new(
            "gpt-4o",
            vec![ConversationItem::text(Role::User, "call echo")],
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

        assert_eq!(
            metrics.output_tokens, 25,
            "status-line throughput should use only cumulative LLM output usage"
        );
    }

    #[tokio::test]
    async fn tool_loop_metrics_fall_back_per_turn_when_usage_is_missing() {
        let provider = MixedUsageProvider;
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(EchoTool);
        let request = PromptRequest::new(
            "gpt-4o",
            vec![ConversationItem::text(Role::User, "call echo")],
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

        let mut fallback_progress = StreamingTokenProgress::new("gpt-4o");
        let fallback_tokens = fallback_progress
            .observe_delta("done", Instant::now())
            .expect("first fallback delta should emit tokens");
        let metrics = completion.metrics.expect("metrics should be recorded");

        assert_eq!(
            metrics.output_tokens,
            20 + fallback_tokens,
            "turns without provider usage should fall back to local LLM output estimates"
        );
    }

    #[tokio::test]
    async fn tool_loop_metrics_duration_excludes_latency_and_tool_execution() {
        let clock = ManualClock::new();
        let provider = TimedToolLoopProvider {
            clock: clock.clone(),
        };
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(ClockAdvanceTool {
            clock: clock.clone(),
        });
        let request = PromptRequest::new(
            "gpt-4o",
            vec![ConversationItem::text(Role::User, "call echo")],
        );
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            executor,
            &cancellation,
            ToolLoopOptions {
                clock: clock.tool_loop_clock(),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let metrics = completion.metrics.expect("metrics should be recorded");

        assert_eq!(
            metrics.duration,
            Duration::from_millis(60),
            "status-line throughput duration should include only LLM generation windows"
        );
    }

    #[tokio::test]
    async fn response_only_metrics_do_not_count_wait_as_generation_duration() {
        let clock = ManualClock::new();
        let provider = ResponseOnlyTimedTextProvider {
            clock: clock.clone(),
        };
        let request =
            PromptRequest::new("gpt-4o", vec![ConversationItem::text(Role::User, "hello")]);
        let cancellation = CancellationToken::new();

        let completion = run_tool_loop(
            &provider,
            request,
            ToolExecutorRegistry::new(),
            &cancellation,
            ToolLoopOptions {
                clock: clock.tool_loop_clock(),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
        .expect("runtime should complete");

        let metrics = completion.metrics.expect("metrics should be recorded");

        assert_eq!(
            metrics.duration,
            Duration::ZERO,
            "response-only providers should not report provider wait time as LLM generation time"
        );
    }

    #[tokio::test]
    async fn mixed_terminating_tool_batch_executes_all_tools_and_continues() {
        let provider = MultiToolBatchProvider::new();
        let mut executor = ToolExecutorRegistry::new();
        executor.insert(ConditionalTerminatingTool);
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call both tools")],
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

        let final_item = completion
            .response
            .items
            .last()
            .expect("final assistant item should be preserved");
        assert_eq!(final_item.role(), Some(Role::Assistant));
        assert_eq!(final_item.text_content(), "done");
        assert_eq!(*provider.calls.lock().unwrap(), 2);
        assert_eq!(completion.response.items.len(), 4);
        assert_eq!(completion.response.items[0].role(), Some(Role::Assistant));
        let ConversationItem::ToolResult {
            call_id: first_call_id,
            ..
        } = &completion.response.items[1]
        else {
            panic!("first tool result should be preserved");
        };
        assert_eq!(first_call_id, "call-terminate");
        let ConversationItem::ToolResult {
            call_id: second_call_id,
            ..
        } = &completion.response.items[2]
        else {
            panic!("second tool result should be preserved");
        };
        assert_eq!(second_call_id, "call-continue");
        assert_eq!(completion.response.items[3].role(), Some(Role::Assistant));

        let request_items = provider
            .request_items
            .lock()
            .expect("request items lock should not poison");
        let second_request_tool_result_ids = request_items[1]
            .iter()
            .filter_map(|item| match item {
                ConversationItem::ToolResult { call_id, .. } => Some(call_id.as_str()),
                _ => None,
            })
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
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

        assert_eq!(completion.response.items.len(), 2);
        assert_eq!(completion.response.items[0].role(), Some(Role::Assistant));
        let assistant_call = completion.response.items[0]
            .tool_calls()
            .next()
            .expect("assistant tool call should be preserved");
        assert_eq!(assistant_call.call_id, "call-1");
        let ConversationItem::ToolResult {
            call_id: result_call_id,
            ..
        } = &completion.response.items[1]
        else {
            panic!("terminating tool result should be preserved");
        };
        assert_eq!(result_call_id, "call-1");
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
            vec![ConversationItem::text(Role::User, "call denied")],
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        );
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        );
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
        let request = PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        );
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
