use std::{future::Future, time::Duration};

use color_eyre::eyre::{Result, WrapErr, eyre};

const SESSION_STORE_BRIDGE_WAIT: Duration = Duration::from_secs(5);

/// `run_session_store_future` 在专用 Tokio worker 上执行同步入口需要的 async store 操作。
///
/// app 启动和 prompt assembly 命令仍处在同步调用栈；每次调用使用独立 runtime 和
/// timeout，让超时 future 被直接丢弃，避免单个慢任务阻塞后续同步入口。
pub(crate) fn run_session_store_future<T, Fut>(
    future_factory: impl FnOnce() -> Fut + Send + 'static,
    runtime_context: &'static str,
) -> Result<T>
where
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    run_session_store_future_with_timeout(
        future_factory,
        SESSION_STORE_BRIDGE_WAIT,
        runtime_context,
    )
}

fn run_session_store_future_with_timeout<T, Fut>(
    future_factory: impl FnOnce() -> Fut + Send + 'static,
    timeout: Duration,
    runtime_context: &'static str,
) -> Result<T>
where
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err(runtime_context)?;
    match runtime.block_on(async move { tokio::time::timeout(timeout, future_factory()).await }) {
        Ok(result) => Ok(result),
        Err(_) => Err(eyre!("{runtime_context} timed out after {timeout:?}")),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    #[test]
    fn run_session_store_future_timeout_does_not_block_later_calls() {
        let long_future_was_dropped = Arc::new(AtomicBool::new(false));
        let dropped_flag = long_future_was_dropped.clone();

        let error = super::run_session_store_future_with_timeout(
            move || async move {
                let _guard = DropFlag(dropped_flag);
                tokio::time::sleep(Duration::from_secs(60)).await;
            },
            Duration::from_millis(1),
            "test session store operation",
        )
        .expect_err("operation should time out");

        assert_eq!(
            error.to_string(),
            "test session store operation timed out after 1ms"
        );
        assert!(
            long_future_was_dropped.load(Ordering::SeqCst),
            "timed-out future should be dropped instead of occupying a serial worker"
        );

        super::run_session_store_future_with_timeout(
            || async { "next job" },
            Duration::from_millis(50),
            "next session store operation",
        )
        .expect("later calls should not wait for the timed-out future");
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}
