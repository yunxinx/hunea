use super::*;

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
        ToolLoopError::Provider(ProviderError::Transport(_))
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

    assert!(matches!(error, ToolLoopError::Cancelled));
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
