use super::support::*;

#[tokio::test]
async fn completed_future_returns_output() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();

    let outcome = run_with_cancellation_grace(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(5),
        async { "done" },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::Completed("done")));
    assert!(!attempt_cancellation.is_cancelled());
}

#[tokio::test]
async fn cancellation_waits_for_repair_inside_grace() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();
    let future_cancellation = attempt_cancellation.clone();
    let repair_observed = Arc::new(AtomicBool::new(false));
    let future_repair_observed = Arc::clone(&repair_observed);
    parent_cancellation.cancel();

    let outcome = run_with_cancellation_grace(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(50),
        async move {
            future_cancellation.cancelled().await;
            future_repair_observed.store(true, Ordering::Relaxed);
            "repaired"
        },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::Completed("repaired")));
    assert!(repair_observed.load(Ordering::Relaxed));
    assert!(attempt_cancellation.is_cancelled());
}

#[tokio::test]
async fn cancellation_stops_waiting_after_repair_grace() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();
    parent_cancellation.cancel();

    let outcome = run_with_cancellation_grace(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(5),
        async {
            std::future::pending::<()>().await;
            "never"
        },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::CancelledAfterGrace));
    assert!(attempt_cancellation.is_cancelled());
}
