use super::*;

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
            permission_handler: Some(std::sync::Arc::new(BlockingPreviewProbePermissionHandler {
                preview: Arc::clone(&captured_preview),
                timer_fired: Arc::clone(&timer_fired),
                timer_fired_before_permission: Arc::clone(&timer_fired_before_permission),
            })),
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
