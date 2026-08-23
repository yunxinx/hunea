use super::support::*;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
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
    let notifier = crate::RuntimeEventNotifier::default();
    let _wake_binding = notifier.bind_callback(move || {
        let _ = wake_sender.send(());
    });
    let sender = ConversationWorkerEventSender::new(sender, notifier);

    run_conversation_worker(
        request,
        fake_provider_lease(),
        executor,
        RuntimeRequestPolicy::default(),
        cancellation,
        None,
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
async fn conversation_retry_reuses_the_same_provider_client_lease() {
    let call_count = Arc::new(AtomicUsize::new(0));
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

    run_conversation_worker(
        request,
        lease,
        ToolExecutorRegistry::new(),
        RuntimeRequestPolicy::new(1, vec![0], 1),
        CancellationToken::new(),
        None,
        sender,
    )
    .await;

    let events = receiver.into_iter().collect::<Vec<_>>();
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
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
