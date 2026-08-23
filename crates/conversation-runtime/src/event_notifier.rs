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

/// `RuntimeEventBinding` 拥有一次 wake callback 注册，并在释放时撤销该 effect。
///
/// 新 callback 替换旧 callback 后，释放旧 binding 不会误删新注册。
#[must_use = "必须持有 binding，wake callback 才保持注册"]
pub struct RuntimeEventBinding {
    callback_slot: Arc<RwLock<Option<Arc<RuntimeEventCallback>>>>,
    owned_callback: Option<Arc<RuntimeEventCallback>>,
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
    /// 绑定 wake callback，并返回拥有该注册的可逆 binding。
    pub fn bind_callback(
        &self,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> RuntimeEventBinding {
        let callback: Arc<RuntimeEventCallback> = Arc::new(callback);
        self.install_callback(Arc::clone(&callback));
        RuntimeEventBinding {
            callback_slot: Arc::clone(&self.callback),
            owned_callback: Some(callback),
        }
    }

    fn install_callback(&self, callback: Arc<RuntimeEventCallback>) {
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

impl RuntimeEventBinding {
    /// `dispose` 幂等撤销当前 binding 拥有的 callback。
    pub fn dispose(&mut self) {
        let Some(owned_callback) = self.owned_callback.take() else {
            return;
        };
        let removed = {
            let mut callback_slot = self
                .callback_slot
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let owns_current_callback = callback_slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &owned_callback));
            owns_current_callback
                .then(|| callback_slot.take())
                .flatten()
        };
        drop(removed);
    }
}

impl Drop for RuntimeEventBinding {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl Drop for RuntimeEventExitNotification {
    fn drop(&mut self) {
        self.notifier.notify();
    }
}
