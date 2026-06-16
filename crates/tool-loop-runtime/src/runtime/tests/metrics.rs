use super::*;

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
    let request = PromptRequest::new("gpt-4o", vec![ConversationItem::text(Role::User, "hello")]);
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
    let request = PromptRequest::new("gpt-4o", vec![ConversationItem::text(Role::User, "hello")]);
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
