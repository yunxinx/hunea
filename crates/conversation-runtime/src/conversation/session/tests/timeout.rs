use super::support::*;

#[tokio::test]
async fn soft_timeout_pauses_while_permission_is_waiting() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();
    let timeout_pause = ConversationTimeoutPause::default();
    let future_timeout_pause = timeout_pause.clone();

    let outcome = run_with_soft_timeout(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(10),
        Duration::from_millis(5),
        timeout_pause,
        async move {
            let permission_wait = future_timeout_pause.pause();
            tokio::time::sleep(Duration::from_millis(30)).await;
            drop(permission_wait);
            "approved"
        },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::Completed("approved")));
    assert!(
        !attempt_cancellation.is_cancelled(),
        "permission waits should not consume the request timeout budget"
    );
}

#[tokio::test]
async fn soft_timeout_cancels_child_and_waits_for_repair() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();
    let future_cancellation = attempt_cancellation.clone();
    let repair_observed = Arc::new(AtomicBool::new(false));
    let future_repair_observed = Arc::clone(&repair_observed);

    let outcome = run_with_soft_timeout(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(10),
        Duration::from_millis(10),
        ConversationTimeoutPause::default(),
        async move {
            future_cancellation.cancelled().await;
            future_repair_observed.store(true, Ordering::Relaxed);
            "repaired"
        },
    )
    .await;

    match outcome {
        TurnAttemptOutcome::TimedOut(value) => assert_eq!(value, "repaired"),
        TurnAttemptOutcome::Completed(_) => {
            panic!("timeout should mark the attempt as timed out")
        }
        TurnAttemptOutcome::TimedOutAfterGrace | TurnAttemptOutcome::CancelledAfterGrace => {
            panic!("future should finish inside the repair grace")
        }
    }
    assert!(repair_observed.load(Ordering::Relaxed));
}

#[tokio::test]
async fn soft_timeout_stops_waiting_after_repair_grace() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();

    let outcome = run_with_soft_timeout(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_millis(5),
        Duration::from_millis(5),
        ConversationTimeoutPause::default(),
        async {
            std::future::pending::<()>().await;
            "never"
        },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::TimedOutAfterGrace));
    assert!(attempt_cancellation.is_cancelled());
}

#[tokio::test]
async fn parent_cancellation_stops_waiting_after_repair_grace() {
    let parent_cancellation = CancellationToken::new();
    let attempt_cancellation = parent_cancellation.child_token();
    parent_cancellation.cancel();

    let outcome = run_with_soft_timeout(
        &parent_cancellation,
        &attempt_cancellation,
        Duration::from_secs(1),
        Duration::from_millis(5),
        ConversationTimeoutPause::default(),
        async {
            std::future::pending::<()>().await;
            "never"
        },
    )
    .await;

    assert!(matches!(outcome, TurnAttemptOutcome::CancelledAfterGrace));
    assert!(attempt_cancellation.is_cancelled());
}
