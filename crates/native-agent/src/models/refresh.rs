use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use mo_core::model_catalog::ModelProviderRefreshEvent;

use super::{ProviderSyncRequest, sync_provider_models_once};

/// `ModelProviderRefreshRuntimeState` 管理 native provider 模型列表刷新 worker。
#[derive(Default)]
pub struct ModelProviderRefreshRuntimeState {
    receiver: Option<Receiver<ModelProviderRefreshEvent>>,
}

impl ModelProviderRefreshRuntimeState {
    pub fn start(&mut self, request: ProviderSyncRequest) {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
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
