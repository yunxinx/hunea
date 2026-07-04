use super::*;

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
async fn usage_total_tokens_become_upstream_context_tokens() {
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

    assert_eq!(completion.upstream_context_tokens, Some(48));
}

#[tokio::test]
async fn request_latency_starts_before_first_stream_frame() {
    let provider = DelayedFirstTokenProvider;
    let request = PromptRequest::new("qwen3", vec![ConversationItem::text(Role::User, "hello")]);
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
