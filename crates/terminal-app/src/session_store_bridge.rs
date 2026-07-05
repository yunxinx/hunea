use std::{
    future::Future,
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};

use color_eyre::eyre::{Result, WrapErr, eyre};
use tokio::runtime::Runtime;

type SessionStoreJob = Box<dyn FnOnce(&Runtime) + Send + 'static>;

const SESSION_STORE_BRIDGE_WAIT: Duration = Duration::from_secs(5);

static SESSION_STORE_BRIDGE: OnceLock<SessionStoreBridge> = OnceLock::new();

#[derive(Clone)]
struct SessionStoreBridge {
    job_sender: mpsc::Sender<SessionStoreJob>,
}

/// `run_session_store_future` 在专用 Tokio worker 上执行同步入口需要的 async store 操作。
///
/// app 启动和 prompt assembly 命令仍处在同步调用栈；这里把 async bridge 固定到一个
/// 长期存活的 worker，避免在全局 `Mutex<Runtime>` 上串行 `block_on`。
pub(crate) fn run_session_store_future<T, Fut>(
    future_factory: impl FnOnce() -> Fut + Send + 'static,
    runtime_context: &'static str,
) -> Result<T>
where
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
{
    let bridge = session_store_bridge(runtime_context)?;
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    bridge
        .job_sender
        .send(Box::new(move |runtime| {
            let result = runtime.block_on(future_factory());
            let _ = result_sender.send(Ok(result));
        }))
        .map_err(|_| eyre!("session store bridge worker stopped"))?;
    receive_session_store_result(result_receiver, SESSION_STORE_BRIDGE_WAIT, runtime_context)
}

fn receive_session_store_result<T>(
    result_receiver: mpsc::Receiver<Result<T>>,
    timeout: Duration,
    runtime_context: &'static str,
) -> Result<T> {
    match result_receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Err(eyre!("{runtime_context} timed out after {timeout:?}"))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(eyre!("session store bridge worker stopped"))
        }
    }
}

fn session_store_bridge(runtime_context: &'static str) -> Result<SessionStoreBridge> {
    if let Some(bridge) = SESSION_STORE_BRIDGE.get() {
        return Ok(bridge.clone());
    }

    let (job_sender, job_receiver) = mpsc::channel::<SessionStoreJob>();
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("hunea-session-store-bridge".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = startup_sender.send(Err(error));
                    return;
                }
            };
            let _ = startup_sender.send(Ok(()));
            while let Ok(job) = job_receiver.recv() {
                job(&runtime);
            }
        })
        .wrap_err(runtime_context)?;
    startup_receiver
        .recv()
        .map_err(|_| eyre!("session store bridge worker stopped during startup"))?
        .wrap_err(runtime_context)?;
    let bridge = SessionStoreBridge { job_sender };
    let _ = SESSION_STORE_BRIDGE.set(bridge.clone());
    SESSION_STORE_BRIDGE
        .get()
        .cloned()
        .ok_or_else(|| eyre!("session store bridge was not initialized"))
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn receive_session_store_result_times_out_while_sender_is_alive() {
        let (_sender, receiver) = mpsc::sync_channel::<color_eyre::eyre::Result<()>>(1);

        let error = super::receive_session_store_result(
            receiver,
            Duration::from_millis(1),
            "test session store operation",
        )
        .expect_err("operation should time out");

        assert_eq!(
            error.to_string(),
            "test session store operation timed out after 1ms"
        );
    }
}
