use std::{
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
};

use runtime_domain::model_catalog::ModelProviderRefreshEvent;

use super::{ProviderSyncRequest, sync_provider_models_once};
use crate::{NotifyingSender, RuntimeEventNotifier};

/// `ModelRefreshWorker` 管理 provider 模型列表刷新 worker。
pub struct ModelRefreshWorker {
    receiver: Option<Receiver<ModelProviderRefreshEvent>>,
    worker_thread: Option<JoinHandle<()>>,
    event_notifier: RuntimeEventNotifier,
}

impl ModelRefreshWorker {
    pub fn new(event_notifier: RuntimeEventNotifier) -> Self {
        Self {
            receiver: None,
            worker_thread: None,
            event_notifier,
        }
    }

    pub fn start(&mut self, request: ProviderSyncRequest) {
        let (sender, receiver) = mpsc::channel();
        let event_notifier = self.event_notifier.clone();
        let sender = NotifyingSender::new(sender, event_notifier.clone());
        let worker_thread = thread::spawn(move || {
            let _exit_notification = event_notifier.notify_on_drop();
            let provider_id = request.provider_id.clone();
            let event = match sync_provider_models_once(&request) {
                Ok(model_ids) => ModelProviderRefreshEvent::Finished {
                    provider_id,
                    model_ids,
                },
                Err(message) => ModelProviderRefreshEvent::Failed {
                    provider_id,
                    message,
                },
            };
            let _ = sender.send(event);
        });
        self.receiver = Some(receiver);
        self.worker_thread = Some(worker_thread);
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn reset_after_clear(&mut self) -> Result<(), String> {
        self.close_and_join()
    }

    /// 关闭刷新 owner，并等待其线程退出，避免 shutdown 后继续产生旧通知。
    pub fn shutdown(&mut self) -> Result<(), String> {
        self.close_and_join()
    }

    pub fn try_recv_event(&mut self) -> Option<ModelProviderRefreshEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                self.receiver = None;
                if let Err(message) = self.join_finished_thread() {
                    let provider_id = match &event {
                        ModelProviderRefreshEvent::Finished { provider_id, .. }
                        | ModelProviderRefreshEvent::Failed { provider_id, .. } => {
                            provider_id.clone()
                        }
                    };
                    return Some(ModelProviderRefreshEvent::Failed {
                        provider_id,
                        message,
                    });
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                let message = self
                    .join_finished_thread()
                    .err()
                    .unwrap_or_else(|| "model refresh stopped before completion".to_string());
                Some(ModelProviderRefreshEvent::Failed {
                    provider_id: String::new(),
                    message,
                })
            }
        }
    }

    fn close_and_join(&mut self) -> Result<(), String> {
        // 先断开 receiver，使线程完成后只能丢弃 payload；`NotifyingSender` 也因此不会
        // 在 owner 已移除后继续发出 wake。
        self.receiver = None;
        self.join_finished_thread()
    }

    fn join_finished_thread(&mut self) -> Result<(), String> {
        let Some(worker_thread) = self.worker_thread.take() else {
            return Ok(());
        };
        worker_thread
            .join()
            .map_err(|_| "model refresh worker thread panicked".to_string())
    }
}

impl Drop for ModelRefreshWorker {
    fn drop(&mut self) {
        let _ = self.close_and_join();
    }
}

impl Default for ModelRefreshWorker {
    fn default() -> Self {
        Self::new(RuntimeEventNotifier::default())
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, time::Duration};

    use runtime_domain::{model_catalog::ProviderSyncRequest, provider::ProviderKind};

    use super::*;
    use crate::RuntimeEventNotifier;

    #[test]
    fn refresh_worker_wakes_after_its_event_is_available() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let _wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);

        worker.start(ProviderSyncRequest {
            provider_id: "anthropic".to_string(),
            kind: ProviderKind::Anthropic,
            display_name: "Anthropic".to_string(),
            base_url: None,
            api_key: None,
            api_key_env: None,
        });

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should wake after sending its event");
        assert!(matches!(
            worker.try_recv_event(),
            Some(ModelProviderRefreshEvent::Failed { provider_id, .. })
                if provider_id == "anthropic"
        ));
        worker.shutdown().expect("refresh worker should shut down");
    }

    #[test]
    fn shutdown_drops_pending_event_before_worker_exit() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let mut wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);

        worker.start(ProviderSyncRequest {
            provider_id: "local".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            display_name: "Local".to_string(),
            base_url: Some("http://127.0.0.1:9/v1".to_string()),
            api_key: None,
            api_key_env: None,
        });

        wake_binding.dispose();
        worker
            .shutdown()
            .expect("refresh worker shutdown should complete");
        assert!(!worker.is_running());
        assert!(worker.try_recv_event().is_none());
        assert!(wake_receiver.try_recv().is_err());
    }
}
