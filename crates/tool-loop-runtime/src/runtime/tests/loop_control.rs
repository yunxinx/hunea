use super::*;

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
