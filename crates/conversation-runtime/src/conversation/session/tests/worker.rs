use super::support::*;
use std::thread;

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
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
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
        executor,
        RuntimeRequestPolicy::default(),
        cancellation,
        ConversationPermissionBroker::default(),
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
