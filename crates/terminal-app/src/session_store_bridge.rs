use std::{
    future::Future,
    sync::{OnceLock, mpsc},
    time::Duration,
};

use color_eyre::eyre::{Result, eyre};

const SESSION_STORE_BRIDGE_WAIT: Duration = Duration::from_secs(5);
type SessionStoreRuntimeJob = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send + 'static>;
static SESSION_STORE_RUNTIME: OnceLock<mpsc::Sender<SessionStoreRuntimeJob>> = OnceLock::new();

/// `run_session_store_future` 在专用 Tokio worker 上执行同步入口需要的 async store 操作。
///
/// app 启动和 prompt assembly 命令仍处在同步调用栈；共享 runtime 避免为每次同步
/// 入口重复创建 Tokio driver，timeout 让超时 future 被直接丢弃。
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
    let (response, receiver) = mpsc::channel();
    session_store_runtime_sender()
        .send(Box::new(move |runtime| {
            let result = runtime
                .block_on(async move { tokio::time::timeout(timeout, future_factory()).await });
            let _ = response.send(result);
        }))
        .map_err(|_| eyre!("{runtime_context} runtime worker stopped"))?;

    match receiver
        .recv()
        .map_err(|_| eyre!("{runtime_context} runtime worker dropped response"))?
    {
        Ok(result) => Ok(result),
        Err(_) => Err(eyre!("{runtime_context} timed out after {timeout:?}")),
    }
}

fn session_store_runtime_sender() -> &'static mpsc::Sender<SessionStoreRuntimeJob> {
    SESSION_STORE_RUNTIME.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<SessionStoreRuntimeJob>();
        std::thread::Builder::new()
            .name("session-store-bridge-runtime".to_string())
            .spawn(move || run_session_store_runtime(receiver))
            .expect("session store runtime thread should start");
        sender
    })
}

fn run_session_store_runtime(receiver: mpsc::Receiver<SessionStoreRuntimeJob>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("session store runtime should initialize");
    while let Ok(job) = receiver.recv() {
        job(&runtime);
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

    #[tokio::test]
    async fn run_session_store_future_can_be_called_inside_tokio_runtime() {
        let value = super::run_session_store_future_with_timeout(
            || async { "loaded" },
            Duration::from_millis(50),
            "nested runtime test",
        )
        .expect("sync bridge should use its worker runtime instead of nesting block_on");

        assert_eq!(value, "loaded");
    }

    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
}
