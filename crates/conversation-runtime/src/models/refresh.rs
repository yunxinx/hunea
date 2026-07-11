use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use runtime_domain::model_catalog::ModelProviderRefreshEvent;

use super::{ProviderSyncRequest, sync_provider_models_once};
use crate::{NotifyingSender, RuntimeEventNotifier};

/// `ModelRefreshWorker` 管理 provider 模型列表刷新 worker。
pub struct ModelRefreshWorker {
    receiver: Option<Receiver<ModelProviderRefreshEvent>>,
    event_notifier: RuntimeEventNotifier,
}

impl ModelRefreshWorker {
    pub fn new(event_notifier: RuntimeEventNotifier) -> Self {
        Self {
            receiver: None,
            event_notifier,
        }
    }

    pub fn start(&mut self, request: ProviderSyncRequest) {
        let (sender, receiver) = mpsc::channel();
        let event_notifier = self.event_notifier.clone();
        let sender = NotifyingSender::new(sender, event_notifier.clone());
        thread::spawn(move || {
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
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn reset_after_clear(&mut self) {
        self.receiver = None;
    }

    pub fn try_recv_event(&mut self) -> Option<ModelProviderRefreshEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                self.receiver = None;
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                Some(ModelProviderRefreshEvent::Failed {
                    provider_id: String::new(),
                    message: "model refresh stopped before completion".to_string(),
                })
            }
        }
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
        notifier
            .install(move || {
                let _ = wake_sender.send(());
            })
            .expect("test notifier should install once");
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
    }
}
