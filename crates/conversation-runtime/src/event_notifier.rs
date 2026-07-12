use std::sync::{Arc, RwLock, mpsc};

type RuntimeEventCallback = dyn Fn() + Send + Sync + 'static;

/// `RuntimeEventNotifier` 把 worker receiver 的就绪状态通知给外层事件循环。
///
/// payload 仍由各 worker 自己的 channel 持有；该类型只负责 wake，避免把调用方的
/// event-loop 类型反向引入 conversation runtime。
#[derive(Clone, Default)]
pub struct RuntimeEventNotifier {
    callback: Arc<RwLock<Option<Arc<RuntimeEventCallback>>>>,
}

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
    /// 替换当前 event loop 的 wake callback。
    ///
    /// notifier clone 共享同一个 callback slot；新 runner 绑定后，后续通知会发往
    /// 最新 callback。旧 callback 在锁外释放，避免其析构影响 notifier lock。
    pub fn replace_callback(&self, callback: impl Fn() + Send + Sync + 'static) {
        let callback: Arc<RuntimeEventCallback> = Arc::new(callback);
        let previous = {
            let mut callback_slot = self
                .callback
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            callback_slot.replace(callback)
        };
        drop(previous);
    }

    /// 通知外层事件循环重新 drain worker receiver。
    pub fn notify(&self) {
        let callback = self
            .callback
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(callback) = callback {
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
