use super::support::*;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

#[test]
fn conversation_worker_reset_waits_until_the_owned_thread_exits() {
    let mut worker = ConversationWorker::new(RuntimeEventNotifier::default());
    let cancellation = CancellationToken::new();
    let thread_cancellation = cancellation.clone();
    let (event_sender, event_receiver) = mpsc::channel();
    let (cancellation_seen_sender, cancellation_seen_receiver) = mpsc::channel();
    let (allow_exit_sender, allow_exit_receiver) = mpsc::channel();
    let worker_thread = thread::spawn(move || {
        while !thread_cancellation.is_cancelled() {
            thread::park_timeout(Duration::from_millis(1));
        }
        cancellation_seen_sender
            .send(())
            .expect("test should observe cancellation");
        allow_exit_receiver
            .recv()
            .expect("test should release the worker thread");
        drop(event_sender);
    });
    worker.receiver = Some(event_receiver);
    worker.worker_thread = Some(worker_thread);
    worker.cancellation = Some(cancellation);

    let (reset_sender, reset_receiver) = mpsc::channel();
    let reset_thread = thread::spawn(move || {
        let cleanup = worker.reset_after_clear();
        reset_sender
            .send((worker, cleanup))
            .expect("test should receive reset result");
    });

    cancellation_seen_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("reset should cancel the worker thread");
    assert!(
        reset_receiver
            .recv_timeout(Duration::from_millis(20))
            .is_err(),
        "reset must not return while its owned thread is still alive"
    );

    allow_exit_sender
        .send(())
        .expect("test should allow the worker to exit");
    let (worker, cleanup) = reset_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("reset should finish after the worker exits");
    cleanup.expect("worker thread should join cleanly");
    reset_thread.join().expect("reset test thread should join");
    assert!(worker.worker_thread.is_none());
    assert!(!worker.is_running());
}

#[tokio::test]
async fn conversation_worker_reports_interrupted_when_pre_cancelled() {
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "local",
        "qwen3",
        ConversationItem::text(Role::User, "hello"),
    );
    let request = PreparedConversationRequest::from_turn(
        &turn,
        vec![ConversationItem::text(Role::User, "hello")],
        None,
        None,
        None,
    );
    let executor = ToolExecutorRegistry::new();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let (sender, receiver) = mpsc::channel();
    let (wake_sender, wake_receiver) = mpsc::channel();
    let notifier = runtime_domain::event_notifier::RuntimeEventNotifier::default();
    let _wake_binding = notifier.bind_callback(move || {
        let _ = wake_sender.send(());
    });
    let sender = ConversationWorkerEventSender::new(sender, notifier);

    run_conversation_worker(
        request,
        fake_provider_lease(),
        executor,
        cancellation,
        conversation_worker_options(
            RuntimeRequestPolicy::default(),
            ExtensionHookRegistry::new(),
        ),
        sender,
    )
    .await;

    assert_eq!(
        receiver.recv().expect("worker should emit an event"),
        ConversationWorkerEvent::progress(ConversationEvent::Interrupted)
    );
    wake_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("worker payload should wake its consumer");
}

#[tokio::test]
async fn empty_request_keeps_the_existing_provider_error_with_an_empty_registry() {
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "fixture",
        "qwen3",
        ConversationItem::text(Role::User, "delivery is intentionally absent"),
    );
    let request = PreparedConversationRequest::from_turn(&turn, Vec::new(), None, None, None);
    let (sender, receiver) = conversation_worker_event_channel();

    run_conversation_worker(
        request,
        fake_provider_lease(),
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(
            RuntimeRequestPolicy::default(),
            ExtensionHookRegistry::new(),
        ),
        sender,
    )
    .await;

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(events.iter().any(|event| matches!(
        event,
        ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })
            if message == "provider fixture received no prompt items"
    )));
    assert!(events.iter().all(|event| !matches!(
        event,
        ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })
            if message.contains("extension hook")
    )));
}

#[tokio::test]
async fn caller_cancellation_during_before_turn_interrupts_before_provider_and_persistence() {
    const PRIVATE_TURN: &str = "PRIVATE_CANCELLED_BEFORE_TURN";
    let root = tempdir_path("before-turn-cancellation");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = Arc::new(
        LocalSessionStore::open_in(root)
            .await
            .expect("local store should open"),
    );
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            ConversationItem::text(Role::User, PRIVATE_TURN),
        ))
        .expect("turn should prepare");
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let hooks = ExtensionHookRegistry::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_for_hook = Arc::clone(&entered);
    let invocation_token = Arc::new(Mutex::new(None::<CancellationToken>));
    let invocation_token_for_hook = Arc::clone(&invocation_token);
    let _registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("cancel-owner").expect("owner id should validate"),
            HookId::try_new("wait-for-cancel").expect("hook id should validate"),
            hook_options(),
            Arc::new(move |_payload, cancellation: CancellationToken| {
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
    let (sender, receiver) = conversation_worker_event_channel();
    let provider = CountingSuccessProvider {
        calls: Arc::clone(&provider_calls),
        observed_items: None,
    };
    let worker = tokio::spawn(run_conversation_worker(
        request,
        lease_for_provider(Arc::new(provider)),
        ToolExecutorRegistry::new(),
        task_cancellation,
        conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
        sender,
    ));

    entered.notified().await;
    cancellation.cancel();
    worker.await.expect("worker task should join");

    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    assert!(
        invocation_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    );
    let events = receiver.try_iter().collect::<Vec<_>>();
    assert_eq!(
        events,
        [ConversationWorkerEvent::progress(
            ConversationEvent::Interrupted
        )]
    );
    assert!(!format!("{events:?}").contains(PRIVATE_TURN));
    let sessions = store
        .list_sessions(
            &ProjectDir::from_work_dir(&work_dir),
            SessionListOptions::default(),
        )
        .await
        .expect("session metadata should remain readable");
    assert!(sessions.is_empty(), "cancellation must precede persistence");
}

#[tokio::test]
async fn registration_disposal_during_before_turn_fails_closed_before_provider() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let hooks = ExtensionHookRegistry::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let entered_for_hook = Arc::clone(&entered);
    let mut registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("dispose-owner").expect("owner id should validate"),
            HookId::try_new("wait-for-dispose").expect("hook id should validate"),
            hook_options(),
            Arc::new(move |_payload, _cancellation| {
                let entered = Arc::clone(&entered_for_hook);
                async move {
                    entered.notify_one();
                    std::future::pending().await
                }
            }),
        )
        .expect("hook should register");
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "local",
        "qwen3",
        ConversationItem::text(Role::User, "private disposal payload"),
    );
    let request = PreparedConversationRequest::from_turn(
        &turn,
        vec![ConversationItem::text(
            Role::User,
            "private disposal payload",
        )],
        None,
        None,
        None,
    );
    let (sender, receiver) = conversation_worker_event_channel();
    let provider = CountingSuccessProvider {
        calls: Arc::clone(&provider_calls),
        observed_items: None,
    };
    let worker = tokio::spawn(run_conversation_worker(
        request,
        lease_for_provider(Arc::new(provider)),
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
        sender,
    ));

    entered.notified().await;
    assert!(registration.dispose());
    worker.await.expect("worker task should join");

    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.as_slice(),
        [ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })]
            if message == "extension hook dispatch failed: phase=before_turn kind=hook_registration_disposed owner=dispose-owner hook=wait-for-dispose"
    ));
    assert!(!format!("{events:?}").contains("private disposal payload"));
}

#[tokio::test]
async fn before_turn_timeout_fails_closed_before_provider() {
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let hooks = ExtensionHookRegistry::new();
    let _registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("timeout-owner").expect("owner id should validate"),
            HookId::try_new("wait-for-timeout").expect("hook id should validate"),
            HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_millis(10))
                .expect("hook options should validate"),
            Arc::new(|_payload, _cancellation| async move { std::future::pending().await }),
        )
        .expect("hook should register");
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "local",
        "qwen3",
        ConversationItem::text(Role::User, "private timeout payload"),
    );
    let request = PreparedConversationRequest::from_turn(
        &turn,
        vec![ConversationItem::text(
            Role::User,
            "private timeout payload",
        )],
        None,
        None,
        None,
    );
    let (sender, receiver) = conversation_worker_event_channel();
    let provider = CountingSuccessProvider {
        calls: Arc::clone(&provider_calls),
        observed_items: None,
    };

    run_conversation_worker(
        request,
        lease_for_provider(Arc::new(provider)),
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
        sender,
    )
    .await;

    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.as_slice(),
        [ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })]
            if message == "extension hook dispatch failed: phase=before_turn kind=hook_timed_out owner=timeout-owner hook=wait-for-timeout"
    ));
    assert!(!format!("{events:?}").contains("private timeout payload"));
}

#[tokio::test]
async fn conversation_retry_reuses_the_same_provider_client_lease() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let hook_invocations = Arc::new(AtomicUsize::new(0));
    let provider = RetryThenSuccessProvider {
        call_count: Arc::clone(&call_count),
    };
    let lease = ProviderClientLease::new(
        "local",
        ProviderKind::OpenAiCompatible,
        Arc::new(provider),
        ProviderPromptCachePolicy::Disabled,
    );
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "local",
        "qwen3",
        ConversationItem::text(Role::User, "hello"),
    );
    let request = PreparedConversationRequest::from_turn(
        &turn,
        vec![ConversationItem::text(Role::User, "hello")],
        None,
        None,
        None,
    );
    let (sender, receiver) = conversation_worker_event_channel();
    let hooks = ExtensionHookRegistry::new();
    let observed_hook_invocations = Arc::clone(&hook_invocations);
    let _registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("retry-owner").expect("owner id should validate"),
            HookId::try_new("once-per-turn").expect("hook id should validate"),
            hook_options(),
            Arc::new(
                move |payload: extension_hook_runtime::BeforeTurnPayload, _cancellation| {
                    observed_hook_invocations.fetch_add(1, Ordering::SeqCst);
                    async move { Ok(BeforeTurnDecision::Continue(payload)) }
                },
            ),
        )
        .expect("hook should register");

    run_conversation_worker(
        request,
        lease,
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(RuntimeRequestPolicy::new(1, vec![0], 1), hooks),
        sender,
    )
    .await;

    let events = receiver.into_iter().collect::<Vec<_>>();
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert_eq!(hook_invocations.load(Ordering::SeqCst), 1);
    assert!(events.iter().any(|event| matches!(
        event,
        ConversationWorkerEvent::Progress(ConversationEvent::Retrying { .. })
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ConversationWorkerEvent::Finished { .. }))
    );
}

#[tokio::test]
async fn before_turn_failure_stops_before_provider_and_persistence() {
    const USER_SENTINEL: &str = "private instruction private user private tool arguments private tool schema private tool result /private/workspace/file private-credential https://private.example/v1 private-session-id private-request-id private-call-id";
    const PRIVATE_VALUES: &[&str] = &[
        "private instruction",
        "private user",
        "private tool arguments",
        "private tool schema",
        "private tool result",
        "/private/workspace/file",
        "private-credential",
        "https://private.example/v1",
        "private-session-id",
        "private-request-id",
        "private-call-id",
    ];
    let root = tempdir_path("before-turn-failure");
    let work_dir = root.join("workspace");
    fs::create_dir_all(&work_dir).expect("work dir should be creatable");
    let store = Arc::new(
        LocalSessionStore::open_in(root)
            .await
            .expect("local store should open"),
    );
    let store_trait: Arc<dyn SessionStore> = store.clone();
    let mut conversation =
        ProviderConversation::with_session_port(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let request = conversation
        .prepare_turn(&runtime_domain::session::ConversationTurnRequest::new(
            "local",
            "qwen3",
            ConversationItem::text(Role::User, USER_SENTINEL),
        ))
        .expect("turn should prepare");
    let provider_calls = Arc::new(AtomicUsize::new(0));
    let provider = CountingSuccessProvider {
        calls: Arc::clone(&provider_calls),
        observed_items: None,
    };
    let hooks = ExtensionHookRegistry::new();
    let _registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("policy-owner").expect("owner id should validate"),
            HookId::try_new("reject-turn").expect("hook id should validate"),
            hook_options(),
            Arc::new(|_payload, _cancellation| async {
                Err::<BeforeTurnDecision, _>(HookFailureKind::Internal)
            }),
        )
        .expect("hook should register");
    let (sender, receiver) = conversation_worker_event_channel();

    run_conversation_worker(
        request,
        lease_for_provider(Arc::new(provider)),
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
        sender,
    )
    .await;

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert_eq!(provider_calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.as_slice(),
        [ConversationWorkerEvent::Progress(ConversationEvent::Failed { message })]
            if message == "extension hook dispatch failed: phase=before_turn kind=hook_internal_failure owner=policy-owner hook=reject-turn"
    ));
    let diagnostic = format!("{events:?}");
    for private in PRIVATE_VALUES {
        assert!(!diagnostic.contains(private), "diagnostic leaked {private}");
    }
    let sessions = store
        .list_sessions(
            &ProjectDir::from_work_dir(&work_dir),
            SessionListOptions::default(),
        )
        .await
        .expect("session metadata should remain readable");
    assert!(
        sessions.is_empty(),
        "before_turn failure must not start persistence"
    );
}

#[tokio::test]
async fn before_turn_transform_is_the_provider_visible_request() {
    let observed_items = Arc::new(Mutex::new(None));
    let provider = CountingSuccessProvider {
        calls: Arc::new(AtomicUsize::new(0)),
        observed_items: Some(Arc::clone(&observed_items)),
    };
    let lease = lease_for_provider(Arc::new(provider));
    let turn = runtime_domain::session::ConversationTurnRequest::new(
        "local",
        "qwen3",
        ConversationItem::text(Role::User, "original provider content"),
    );
    let request = PreparedConversationRequest::from_turn(
        &turn,
        vec![ConversationItem::text(
            Role::User,
            "original provider content",
        )],
        None,
        None,
        None,
    );
    let hooks = ExtensionHookRegistry::new();
    let _registration = hooks
        .register_before_turn(
            HookOwnerId::try_new("transform-owner").expect("owner id should validate"),
            HookId::try_new("replace-items").expect("hook id should validate"),
            hook_options(),
            Arc::new(
                |payload: extension_hook_runtime::BeforeTurnPayload, _cancellation| async move {
                    let payload = payload
                        .replace_items(vec![ConversationItem::text(
                            Role::User,
                            "transformed provider content",
                        )])
                        .map_err(|_| HookFailureKind::InvalidInput)?;
                    Ok(BeforeTurnDecision::Continue(payload))
                },
            ),
        )
        .expect("hook should register");
    let (sender, receiver) = conversation_worker_event_channel();

    run_conversation_worker(
        request,
        lease,
        ToolExecutorRegistry::new(),
        CancellationToken::new(),
        conversation_worker_options(RuntimeRequestPolicy::default(), hooks),
        sender,
    )
    .await;

    let events = receiver.try_iter().collect::<Vec<_>>();
    assert!(matches!(
        events.last(),
        Some(ConversationWorkerEvent::Finished { .. })
    ));
    let observed = observed_items
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("provider should observe one request");
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].text_content(), "transformed provider content");
}

fn hook_options() -> HookRegistrationOptions {
    HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_secs(1))
        .expect("hook options should validate")
}

fn lease_for_provider(provider: Arc<dyn ProviderClient>) -> ProviderClientLease {
    ProviderClientLease::new(
        "local",
        ProviderKind::OpenAiCompatible,
        provider,
        ProviderPromptCachePolicy::Disabled,
    )
}

struct CountingSuccessProvider {
    calls: Arc<AtomicUsize>,
    observed_items: Option<Arc<Mutex<Option<Vec<ConversationItem>>>>>,
}

impl ProviderClient for CountingSuccessProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        _sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(observed_items) = &self.observed_items {
            *observed_items
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request.items.clone());
        }
        Box::pin(async {
            Ok(PromptCompletion::new(
                vec![ConversationItem::text(Role::Assistant, "done")],
                provider_protocol::FinishReason::Stop,
                None,
            ))
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

struct RetryThenSuccessProvider {
    call_count: Arc<AtomicUsize>,
}

impl ProviderClient for RetryThenSuccessProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        _sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let attempt = self.call_count.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                return Err(ProviderError::Transport(
                    "retryable fixture failure".to_string(),
                ));
            }
            Ok(PromptCompletion::new(
                vec![ConversationItem::text(Role::Assistant, "done")],
                provider_protocol::FinishReason::Stop,
                None,
            ))
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
