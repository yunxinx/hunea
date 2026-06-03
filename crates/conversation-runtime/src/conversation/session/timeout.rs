use std::{future::Future, time::Duration};

use tokio_util::sync::CancellationToken;

use crate::conversation::ConversationTimeoutPause;

pub(super) enum TurnAttemptOutcome<T> {
    Completed(T),
    TimedOut(T),
    TimedOutAfterGrace,
    CancelledAfterGrace,
}

pub(super) async fn run_with_soft_timeout<T>(
    parent_cancellation: &CancellationToken,
    attempt_cancellation: &CancellationToken,
    timeout: Duration,
    repair_grace: Duration,
    timeout_pause: ConversationTimeoutPause,
    future: impl Future<Output = T>,
) -> TurnAttemptOutcome<T> {
    tokio::pin!(future);
    let mut timeout_remaining = timeout;
    let mut pause_receiver = timeout_pause.subscribe();

    loop {
        if *pause_receiver.borrow_and_update() > 0 {
            tokio::select! {
                output = &mut future => return TurnAttemptOutcome::Completed(output),
                _ = parent_cancellation.cancelled() => {
                    attempt_cancellation.cancel();
                    let repair_grace = tokio::time::sleep(repair_grace);
                    tokio::pin!(repair_grace);
                    tokio::select! {
                        output = &mut future => return TurnAttemptOutcome::Completed(output),
                        _ = &mut repair_grace => return TurnAttemptOutcome::CancelledAfterGrace,
                    }
                }
                _ = wait_until_timeout_unpaused(&mut pause_receiver) => {}
            }
            continue;
        }

        let active_started_at = std::time::Instant::now();
        let timeout_sleep = tokio::time::sleep(timeout_remaining);
        tokio::pin!(timeout_sleep);

        tokio::select! {
            output = &mut future => return TurnAttemptOutcome::Completed(output),
            _ = parent_cancellation.cancelled() => {
                attempt_cancellation.cancel();
                let repair_grace = tokio::time::sleep(repair_grace);
                tokio::pin!(repair_grace);
                tokio::select! {
                    output = &mut future => return TurnAttemptOutcome::Completed(output),
                    _ = &mut repair_grace => return TurnAttemptOutcome::CancelledAfterGrace,
                }
            }
            _ = wait_until_timeout_paused(&mut pause_receiver) => {
                timeout_remaining = timeout_remaining
                    .saturating_sub(active_started_at.elapsed());
            }
            _ = &mut timeout_sleep => {
                attempt_cancellation.cancel();
                let repair_grace = tokio::time::sleep(repair_grace);
                tokio::pin!(repair_grace);
                tokio::select! {
                    output = &mut future => return TurnAttemptOutcome::TimedOut(output),
                    _ = parent_cancellation.cancelled() => return TurnAttemptOutcome::CancelledAfterGrace,
                    _ = &mut repair_grace => return TurnAttemptOutcome::TimedOutAfterGrace,
                }
            }
        }
    }
}

async fn wait_until_timeout_paused(receiver: &mut tokio::sync::watch::Receiver<usize>) {
    loop {
        if *receiver.borrow_and_update() > 0 {
            return;
        }
        if receiver.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn wait_until_timeout_unpaused(receiver: &mut tokio::sync::watch::Receiver<usize>) {
    loop {
        if *receiver.borrow_and_update() == 0 {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
