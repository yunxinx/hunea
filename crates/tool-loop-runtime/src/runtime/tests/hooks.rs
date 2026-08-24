use extension_hook_runtime::{
    AfterToolResultDecision, AfterToolResultPayload, BeforeToolExecuteDecision,
    BeforeToolExecutePayload, ExtensionHookRegistry, HookFailureKind, HookId, HookOwnerId,
    HookPriority, HookRegistrationOptions, HookRejectionKind,
};

use super::*;

fn hook_options(priority: i32) -> HookRegistrationOptions {
    HookRegistrationOptions::try_new(HookPriority::new(priority), Duration::from_millis(100))
        .expect("hook options should validate")
}

fn hook_owner() -> HookOwnerId {
    HookOwnerId::try_new("test-policy").expect("hook owner should validate")
}

fn hook_id(value: &str) -> HookId {
    HookId::try_new(value).expect("hook id should validate")
}

struct PermissionOrderingProbe {
    permission_requested: Arc<AtomicBool>,
}

impl ToolPermissionHandler for PermissionOrderingProbe {
    fn request_permission<'a>(
        &'a self,
        _request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        self.permission_requested.store(true, Ordering::SeqCst);
        Box::pin(async { ToolPermissionDecision::Allow })
    }
}

struct ExecutedProbeTool {
    executed: Arc<AtomicBool>,
}

impl Tool for ExecutedProbeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_input_schema(serde_json::json!({
                "description": "private-tool-schema",
                "type": "object"
            }))
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.executed.store(true, Ordering::SeqCst);
        Box::pin(async move { ToolResult::success(call.call_id, "raw-tool-result-secret") })
    }
}

#[tokio::test]
async fn before_tool_reject_runs_after_permission_and_skips_tool_and_after_hooks() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let permission_requested = Arc::new(AtomicBool::new(false));
    let executed = Arc::new(AtomicBool::new(false));
    let after_ran = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let permission_for_hook = Arc::clone(&permission_requested);
    let before_registration = hooks
        .register_before_tool_execute(
            hook_owner(),
            hook_id("reject"),
            hook_options(0),
            Arc::new(move |_: BeforeToolExecutePayload, _| {
                let permission_requested = Arc::clone(&permission_for_hook);
                async move {
                    assert!(
                        permission_requested.load(Ordering::SeqCst),
                        "before-tool hook must run after permission succeeds"
                    );
                    Ok(BeforeToolExecuteDecision::Reject(
                        HookRejectionKind::PolicyDenied,
                    ))
                }
            }),
        )
        .unwrap();
    let after_ran_for_hook = Arc::clone(&after_ran);
    let after_registration = hooks
        .register_after_tool_result(
            hook_owner(),
            hook_id("must-not-run"),
            hook_options(0),
            Arc::new(move |payload: AfterToolResultPayload, _| {
                let after_ran = Arc::clone(&after_ran_for_hook);
                async move {
                    after_ran.store(true, Ordering::SeqCst);
                    Ok(AfterToolResultDecision::Continue(payload))
                }
            }),
        )
        .unwrap();
    let mut events = Vec::new();

    let completion = run_tool_loop(
        &provider,
        PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        ),
        executor,
        &CancellationToken::new(),
        ToolLoopOptions {
            permission_handler: Some(Arc::new(PermissionOrderingProbe {
                permission_requested: Arc::clone(&permission_requested),
            })),
            extension_hooks: hooks,
            ..ToolLoopOptions::default()
        },
        |event| events.push(event),
    )
    .await
    .expect("policy rejection should return a fixed tool result");

    assert!(permission_requested.load(Ordering::SeqCst));
    assert!(!executed.load(Ordering::SeqCst));
    assert!(!after_ran.load(Ordering::SeqCst));
    let tool_result = completion
        .response
        .items
        .iter()
        .find_map(|item| match item {
            ConversationItem::ToolResult {
                content, is_error, ..
            } => Some((content, is_error)),
            _ => None,
        })
        .expect("policy result should be appended");
    assert!(tool_result.1);
    assert_eq!(
        tool_result
            .0
            .iter()
            .filter_map(|content| match content {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        "Tool execution rejected by extension policy"
    );
    assert!(!format!("{events:?}").contains("raw-tool-result-secret"));
    drop((before_registration, after_registration));
}

#[tokio::test]
async fn after_tool_transform_precedes_provider_and_activity_projection() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let registration = hooks
        .register_after_tool_result(
            hook_owner(),
            hook_id("transform"),
            hook_options(0),
            Arc::new(|payload: AfterToolResultPayload, _| async move {
                let replacement = ToolResult::error(
                    payload.result().call_id().to_string(),
                    "transformed-tool-result",
                );
                Ok(AfterToolResultDecision::Continue(
                    payload
                        .replace_result(replacement)
                        .expect("call identity should be preserved"),
                ))
            }),
        )
        .unwrap();
    let mut events = Vec::new();

    let completion = run_tool_loop(
        &provider,
        PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        ),
        executor,
        &CancellationToken::new(),
        ToolLoopOptions {
            permission_handler: Some(Arc::new(PermissionOrderingProbe {
                permission_requested: Arc::new(AtomicBool::new(false)),
            })),
            extension_hooks: hooks,
            ..ToolLoopOptions::default()
        },
        |event| events.push(event),
    )
    .await
    .expect("transformed result should continue the loop");

    assert!(executed.load(Ordering::SeqCst));
    let provider_projection = completion
        .response
        .items
        .iter()
        .find_map(|item| match item {
            ConversationItem::ToolResult {
                content, is_error, ..
            } => Some((content, is_error)),
            _ => None,
        })
        .expect("tool result should be appended");
    assert!(provider_projection.1);
    assert!(matches!(
        &provider_projection.0[0],
        ContentBlock::Text(text) if text.contains("transformed-tool-result")
    ));
    let activity_projection = events
        .iter()
        .find_map(|event| match event {
            ToolLoopProgress::ToolActivityUpdated { update } => update.content.as_ref(),
            _ => None,
        })
        .expect("activity should be projected from transformed result");
    assert!(
        activity_projection
            .iter()
            .any(|content| matches!(content, runtime_domain::session::RuntimeToolActivityContent::Text(text) if text.contains("transformed-tool-result")))
    );
    let context_projection = events
        .iter()
        .find_map(|event| match event {
            ToolLoopProgress::ProviderContextItem {
                item: ConversationItem::ToolResult { content, .. },
            } => Some(content),
            _ => None,
        })
        .expect("provider context should receive the transformed result");
    assert!(matches!(
        &context_projection[0],
        ContentBlock::Text(text) if text.contains("transformed-tool-result")
    ));
    assert!(!format!("{events:?}").contains("raw-tool-result-secret"));
    drop(registration);
}

#[tokio::test]
async fn after_hook_failure_stops_before_result_projection() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let registration = hooks
        .register_after_tool_result(
            hook_owner(),
            hook_id("fail"),
            hook_options(0),
            Arc::new(|_: AfterToolResultPayload, _| async move { Err(HookFailureKind::Internal) }),
        )
        .unwrap();
    let mut events = Vec::new();

    let error = run_tool_loop(
        &provider,
        PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(Role::User, "call echo")],
        ),
        executor,
        &CancellationToken::new(),
        ToolLoopOptions {
            permission_handler: Some(Arc::new(PermissionOrderingProbe {
                permission_requested: Arc::new(AtomicBool::new(false)),
            })),
            extension_hooks: hooks,
            ..ToolLoopOptions::default()
        },
        |event| events.push(event),
    )
    .await
    .expect_err("hook failure should fail closed");

    assert!(executed.load(Ordering::SeqCst));
    assert!(matches!(error, ToolLoopError::ExtensionHook { .. }));
    assert!(!events.iter().any(|event| {
        matches!(
            event,
            ToolLoopProgress::ToolActivityUpdated { .. }
                | ToolLoopProgress::ProviderContextItem {
                    item: ConversationItem::ToolResult { .. }
                }
        )
    }));
    let diagnostic = format!("{error:?} {error}");
    for private in [
        "raw-tool-result-secret",
        "private-tool-schema",
        "call-1",
        r#"{"text":"hi"}"#,
    ] {
        assert!(!diagnostic.contains(private));
    }
    drop(registration);
}

#[tokio::test]
async fn caller_cancellation_during_before_hook_stops_tool_and_result_projection() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_for_hook = Arc::clone(&entered);
    let invocation_token = Arc::new(Mutex::new(None::<CancellationToken>));
    let invocation_token_for_hook = Arc::clone(&invocation_token);
    let _registration = hooks
        .register_before_tool_execute(
            hook_owner(),
            hook_id("wait-for-cancel"),
            hook_options(0),
            Arc::new(move |_: BeforeToolExecutePayload, cancellation| {
                *invocation_token_for_hook
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancellation);
                let entered = Arc::clone(&entered_for_hook);
                async move {
                    entered.notify_one();
                    std::future::pending().await
                }
            }),
        )
        .expect("hook should register");
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        let mut events = Vec::new();
        let completion = run_tool_loop(
            &provider,
            PromptRequest::new(
                "qwen3",
                vec![ConversationItem::text(Role::User, "private cancel request")],
            ),
            executor,
            &task_cancellation,
            ToolLoopOptions {
                permission_handler: Some(Arc::new(PermissionOrderingProbe {
                    permission_requested: Arc::new(AtomicBool::new(false)),
                })),
                extension_hooks: hooks,
                ..ToolLoopOptions::default()
            },
            |event| events.push(event),
        )
        .await;
        (completion, events)
    });

    entered.notified().await;
    cancellation.cancel();
    let (completion, events) = task.await.expect("tool loop task should join");

    assert!(matches!(completion, Err(ToolLoopError::Cancelled)));
    assert!(!executed.load(Ordering::SeqCst));
    assert!(
        invocation_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    );
    assert!(!has_result_projection(&events));
}

#[tokio::test]
async fn registration_disposal_during_before_hook_stops_tool_and_result_projection() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_for_hook = Arc::clone(&entered);
    let mut registration = hooks
        .register_before_tool_execute(
            hook_owner(),
            hook_id("wait-for-disposal"),
            hook_options(0),
            Arc::new(move |_: BeforeToolExecutePayload, _| {
                let entered = Arc::clone(&entered_for_hook);
                async move {
                    entered.notify_one();
                    std::future::pending::<Result<BeforeToolExecuteDecision, HookFailureKind>>()
                        .await
                }
            }),
        )
        .expect("hook should register");
    let task = tokio::spawn(async move {
        let mut events = Vec::new();
        let completion = run_tool_loop(
            &provider,
            PromptRequest::new(
                "qwen3",
                vec![ConversationItem::text(
                    Role::User,
                    "private disposal request",
                )],
            ),
            executor,
            &CancellationToken::new(),
            ToolLoopOptions {
                permission_handler: Some(Arc::new(PermissionOrderingProbe {
                    permission_requested: Arc::new(AtomicBool::new(false)),
                })),
                extension_hooks: hooks,
                ..ToolLoopOptions::default()
            },
            |event| events.push(event),
        )
        .await;
        (completion, events)
    });

    entered.notified().await;
    assert!(registration.dispose());
    let (completion, events) = task.await.expect("tool loop task should join");

    assert!(matches!(
        completion,
        Err(ToolLoopError::ExtensionHook { source })
            if source.kind() == extension_hook_runtime::HookDispatchErrorKind::RegistrationDisposed
    ));
    assert!(!executed.load(Ordering::SeqCst));
    assert!(!has_result_projection(&events));
}

#[tokio::test]
async fn before_hook_timeout_stops_tool_and_result_projection() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let _registration = hooks
        .register_before_tool_execute(
            hook_owner(),
            hook_id("wait-for-timeout"),
            HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_millis(10))
                .expect("hook options should validate"),
            Arc::new(|_: BeforeToolExecutePayload, _| async move {
                std::future::pending::<Result<BeforeToolExecuteDecision, HookFailureKind>>().await
            }),
        )
        .expect("hook should register");
    let mut events = Vec::new();

    let completion = run_tool_loop(
        &provider,
        PromptRequest::new(
            "qwen3",
            vec![ConversationItem::text(
                Role::User,
                "private timeout request",
            )],
        ),
        executor,
        &CancellationToken::new(),
        ToolLoopOptions {
            permission_handler: Some(Arc::new(PermissionOrderingProbe {
                permission_requested: Arc::new(AtomicBool::new(false)),
            })),
            extension_hooks: hooks,
            ..ToolLoopOptions::default()
        },
        |event| events.push(event),
    )
    .await;

    assert!(matches!(
        completion,
        Err(ToolLoopError::ExtensionHook { source })
            if source.kind() == extension_hook_runtime::HookDispatchErrorKind::TimedOut
    ));
    assert!(!executed.load(Ordering::SeqCst));
    assert!(!has_result_projection(&events));
}

#[tokio::test]
async fn caller_cancellation_during_after_hook_hides_the_raw_tool_result() {
    let provider = FakeProvider {
        calls: Mutex::new(0),
    };
    let executed = Arc::new(AtomicBool::new(false));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(ExecutedProbeTool {
        executed: Arc::clone(&executed),
    });
    let hooks = ExtensionHookRegistry::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_for_hook = Arc::clone(&entered);
    let invocation_token = Arc::new(Mutex::new(None::<CancellationToken>));
    let invocation_token_for_hook = Arc::clone(&invocation_token);
    let _registration = hooks
        .register_after_tool_result(
            hook_owner(),
            hook_id("wait-after-result"),
            hook_options(0),
            Arc::new(move |_: AfterToolResultPayload, cancellation| {
                *invocation_token_for_hook
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancellation);
                let entered = Arc::clone(&entered_for_hook);
                async move {
                    entered.notify_one();
                    std::future::pending().await
                }
            }),
        )
        .expect("hook should register");
    let cancellation = CancellationToken::new();
    let task_cancellation = cancellation.clone();
    let task = tokio::spawn(async move {
        let mut events = Vec::new();
        let completion = run_tool_loop(
            &provider,
            PromptRequest::new(
                "qwen3",
                vec![ConversationItem::text(Role::User, "private cancel request")],
            ),
            executor,
            &task_cancellation,
            ToolLoopOptions {
                permission_handler: Some(Arc::new(PermissionOrderingProbe {
                    permission_requested: Arc::new(AtomicBool::new(false)),
                })),
                extension_hooks: hooks,
                ..ToolLoopOptions::default()
            },
            |event| events.push(event),
        )
        .await;
        (completion, events)
    });

    entered.notified().await;
    cancellation.cancel();
    let (completion, events) = task.await.expect("tool loop task should join");

    assert!(matches!(completion, Err(ToolLoopError::Cancelled)));
    assert!(executed.load(Ordering::SeqCst));
    assert!(
        invocation_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    );
    assert!(!has_result_projection(&events));
    assert!(!format!("{events:?}").contains("raw-tool-result-secret"));
}

fn has_result_projection(events: &[ToolLoopProgress]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            ToolLoopProgress::ToolActivityUpdated { .. }
                | ToolLoopProgress::ProviderContextItem {
                    item: ConversationItem::ToolResult { .. }
                }
        )
    })
}
