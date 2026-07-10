use super::support::*;

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
    notifier
        .install(move || {
            let _ = wake_sender.send(());
        })
        .expect("test notifier should install once");
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
