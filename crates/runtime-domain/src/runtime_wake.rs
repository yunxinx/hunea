use std::sync::Arc;

type RuntimeWakeCallback = dyn Fn() + Send + Sync + 'static;

/// Runtime producer 用于通知宿主重新观察已发布事实的无 payload port。
///
/// 该类型只表达通知能力，不携带 event、instruction 或 control metadata。具体的
/// event loop adapter 由消费方决定，因此 domain crate 不依赖 TUI 或 async runtime。
#[derive(Clone)]
pub struct RuntimeWake {
    callback: Arc<RuntimeWakeCallback>,
}

impl RuntimeWake {
    /// 创建一个可跨线程调用的 wake callback。
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    /// 触发宿主重新观察 runtime facts。
    pub fn wake(&self) {
        (self.callback)();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::RuntimeWake;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn wake_port_is_send_and_sync() {
        assert_send_sync::<RuntimeWake>();
    }

    #[test]
    fn cloned_wake_handles_share_one_notification_port() {
        let notifications = Arc::new(AtomicUsize::new(0));
        let callback_notifications = Arc::clone(&notifications);
        let wake = RuntimeWake::new(move || {
            callback_notifications.fetch_add(1, Ordering::SeqCst);
        });
        let cloned = wake.clone();

        wake.wake();
        cloned.wake();

        assert_eq!(notifications.load(Ordering::SeqCst), 2);
    }
}
