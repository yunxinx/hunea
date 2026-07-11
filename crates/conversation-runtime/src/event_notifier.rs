use std::sync::{Arc, OnceLock, mpsc};

type RuntimeEventCallback = dyn Fn() + Send + Sync + 'static;

/// `RuntimeEventNotifier` 把 worker receiver 的就绪状态通知给外层事件循环。
///
/// payload 仍由各 worker 自己的 channel 持有；该类型只负责 wake，避免把调用方的
/// event-loop 类型反向引入 conversation runtime。
#[derive(Clone, Default)]
pub struct RuntimeEventNotifier {
    callback: Arc<OnceLock<Arc<RuntimeEventCallback>>>,
}

/// 重复安装 runtime event callback 时返回的错误。
#[derive(Debug, thiserror::Error)]
#[error("runtime event notifier callback is already installed")]
pub struct RuntimeEventNotifierInstallError;

/// worker scope 退出时补发一次通知，使 receiver disconnect 能被立即观察。
#[must_use = "必须持有到 worker scope 结束，才能在退出时发送通知"]
pub struct RuntimeEventExitNotification {
    notifier: RuntimeEventNotifier,
}

/// `NotifyingSender` 保证 payload 成功入队后才通知外层事件循环。
#[derive(Clone)]
pub struct NotifyingSender<T> {
    sender: mpsc::Sender<T>,
    notifier: RuntimeEventNotifier,
}

impl<T> NotifyingSender<T> {
    pub fn new(sender: mpsc::Sender<T>, notifier: RuntimeEventNotifier) -> Self {
        Self { sender, notifier }
    }

    pub fn send(&self, payload: T) -> Result<(), mpsc::SendError<T>> {
        self.sender.send(payload)?;
        self.notifier.notify();
        Ok(())
    }

    pub fn notify_on_drop(&self) -> RuntimeEventExitNotification {
        self.notifier.notify_on_drop()
    }
}

impl RuntimeEventNotifier {
    /// 安装唯一的 wake callback。
    pub fn install(
        &self,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> Result<(), RuntimeEventNotifierInstallError> {
        self.callback
            .set(Arc::new(callback))
            .map_err(|_| RuntimeEventNotifierInstallError)
    }

    /// 通知外层事件循环重新 drain worker receiver。
    pub fn notify(&self) {
        if let Some(callback) = self.callback.get() {
            callback();
        }
    }

    /// 创建一个在 Drop 时通知外层事件循环的 worker scope guard。
    pub fn notify_on_drop(&self) -> RuntimeEventExitNotification {
        RuntimeEventExitNotification {
            notifier: self.clone(),
        }
    }
}

impl Drop for RuntimeEventExitNotification {
    fn drop(&mut self) {
        self.notifier.notify();
    }
}
