use std::{future::Future, time::Duration};

use tokio_util::sync::CancellationToken;

pub(super) enum TurnAttemptOutcome<T> {
    Completed(T),
    CancelledAfterGrace,
}

/// `run_with_cancellation_grace` 等待 turn future 完成；父级取消后给予一段
/// 修复宽限，让工具回灌等收尾工作有机会完成，超过宽限即放弃。
pub(super) async fn run_with_cancellation_grace<T>(
    parent_cancellation: &CancellationToken,
    attempt_cancellation: &CancellationToken,
    repair_grace: Duration,
    future: impl Future<Output = T>,
) -> TurnAttemptOutcome<T> {
    tokio::pin!(future);

    tokio::select! {
        output = &mut future => TurnAttemptOutcome::Completed(output),
        _ = parent_cancellation.cancelled() => {
            attempt_cancellation.cancel();
            let repair_grace = tokio::time::sleep(repair_grace);
            tokio::pin!(repair_grace);
            tokio::select! {
                output = &mut future => TurnAttemptOutcome::Completed(output),
                _ = &mut repair_grace => TurnAttemptOutcome::CancelledAfterGrace,
            }
        }
    }
}
