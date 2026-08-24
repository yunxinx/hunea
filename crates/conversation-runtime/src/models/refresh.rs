use std::{
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
};

use runtime_domain::{
    event_notifier::{NotifyingSender, RuntimeEventNotifier},
    model_catalog::ModelProviderRefreshEvent,
};

use super::ProviderSyncRequest;
use crate::ProviderClientLease;

/// 模型列表请求的 HTTP idle timeout 与 total timeout 上限。
pub const MODEL_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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

    pub fn start(
        &mut self,
        request: ProviderSyncRequest,
        provider_lease: ProviderClientLease,
    ) -> Result<(), String> {
        self.start_with_total_timeout(request, provider_lease, MODEL_LIST_TIMEOUT)
    }

    fn start_with_total_timeout(
        &mut self,
        request: ProviderSyncRequest,
        provider_lease: ProviderClientLease,
        total_timeout: std::time::Duration,
    ) -> Result<(), String> {
        if self.is_running() {
            return Err("model refresh is already running".to_string());
        }
        let (sender, receiver) = mpsc::channel();
        let event_notifier = self.event_notifier.clone();
        let sender = NotifyingSender::new(sender, event_notifier.clone());
        let worker_thread = thread::spawn(move || {
            let _exit_notification = event_notifier.notify_on_drop();
            let provider_id = request.provider_id.clone();
            let event = match list_provider_models(provider_lease, total_timeout) {
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
        Ok(())
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

fn list_provider_models(
    provider_lease: ProviderClientLease,
    total_timeout: std::time::Duration,
) -> Result<Vec<String>, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("start model refresh runtime: {error}"))?;
    runtime.block_on(async move {
        tokio::time::timeout(total_timeout, provider_lease.client().list_models())
            .await
            .map_err(|_| "model sync timed out".to_string())?
            .map(|models| models.into_iter().map(|model| model.id).collect())
            .map_err(|error| model_refresh_failure_message(&error))
    })
}

fn model_refresh_failure_message(error: &provider_protocol::ProviderError) -> String {
    match error {
        provider_protocol::ProviderError::Transport(_) => {
            "provider model listing transport failed".to_string()
        }
        provider_protocol::ProviderError::Protocol(_) => {
            "provider model listing response was invalid".to_string()
        }
        provider_protocol::ProviderError::Provider { status, .. } => status.map_or_else(
            || "provider model listing failed".to_string(),
            |status| format!("provider model listing failed with HTTP {status}"),
        ),
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
    use provider_protocol::{
        ModelDescriptor, PromptCompletion, PromptRequest, ProviderCapabilities, ProviderClient,
        ProviderError, ProviderFuture, StreamEventSink,
    };
    use runtime_domain::model_catalog::ProviderSyncRequest;
    use std::{
        future,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::Duration,
    };

    use super::*;
    use runtime_domain::event_notifier::RuntimeEventNotifier;

    #[test]
    fn refresh_worker_lists_models_and_wakes_after_the_event_is_available() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let _wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);

        worker
            .start(
                request("local"),
                lease(Arc::new(SuccessProvider {
                    model_ids: vec!["qwen3".to_string(), "qwen3-coder".to_string()],
                })),
            )
            .expect("refresh should start");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("worker should wake after sending its event");
        assert!(matches!(
            worker.try_recv_event(),
            Some(ModelProviderRefreshEvent::Finished {
                provider_id,
                model_ids,
            }) if provider_id == "local"
                && model_ids == vec!["qwen3".to_string(), "qwen3-coder".to_string()]
        ));
        worker.shutdown().expect("refresh worker should shut down");
    }

    #[test]
    fn refresh_worker_projects_provider_failure() {
        let sentinel = "https://private.invalid/model-list-instruction-sentinel";
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let _wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);

        worker
            .start(request("local"), lease(Arc::new(FailingProvider)))
            .expect("refresh should start");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("provider failure should wake the owner");
        assert!(matches!(
            worker.try_recv_event(),
            Some(ModelProviderRefreshEvent::Failed { provider_id, message })
                if provider_id == "local"
                    && message == "provider model listing transport failed"
                    && !message.contains(sentinel)
        ));
    }

    #[test]
    fn refresh_worker_applies_a_total_timeout() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let _wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);

        worker
            .start_with_total_timeout(
                request("slow"),
                lease(Arc::new(PendingProvider)),
                Duration::from_millis(20),
            )
            .expect("refresh should start");

        wake_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("timed out refresh should wake the owner");
        assert!(matches!(
            worker.try_recv_event(),
            Some(ModelProviderRefreshEvent::Failed { provider_id, message })
                if provider_id == "slow" && message == "model sync timed out"
        ));
    }

    #[test]
    fn shutdown_drops_pending_event_before_joining_worker() {
        let (wake_sender, wake_receiver) = mpsc::channel();
        let notifier = RuntimeEventNotifier::default();
        let mut wake_binding = notifier.bind_callback(move || {
            let _ = wake_sender.send(());
        });
        let mut worker = ModelRefreshWorker::new(notifier);
        let (started_sender, started_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();

        worker
            .start(
                request("local"),
                lease(Arc::new(BlockingProvider {
                    started: Mutex::new(Some(started_sender)),
                    release: Mutex::new(release_receiver),
                })),
            )
            .expect("refresh should start");
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("provider should enter list_models");
        let second_start = worker.start(
            request("second"),
            lease(Arc::new(SuccessProvider {
                model_ids: vec!["must-not-start".to_string()],
            })),
        );
        assert_eq!(
            second_start,
            Err("model refresh is already running".to_string())
        );

        wake_binding.dispose();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let shutdown_thread = thread::spawn(move || {
            let result = worker.shutdown();
            shutdown_sender
                .send((worker, result))
                .expect("test should receive the shutdown worker");
        });
        assert!(
            shutdown_receiver
                .recv_timeout(Duration::from_millis(20))
                .is_err()
        );
        release_sender
            .send(())
            .expect("test should release model listing");
        let (mut worker, shutdown_result) = shutdown_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown should finish after provider release");
        shutdown_result.expect("refresh worker shutdown should complete");
        shutdown_thread
            .join()
            .expect("shutdown test thread should join");
        assert!(!worker.is_running());
        assert!(worker.try_recv_event().is_none());
        assert!(wake_receiver.try_recv().is_err());
    }

    #[test]
    fn consuming_terminal_event_joins_worker_and_releases_lease() {
        let drop_count = Arc::new(AtomicUsize::new(0));
        let mut worker = ModelRefreshWorker::default();
        worker
            .start(
                request("local"),
                lease(Arc::new(DropTrackedProvider {
                    drop_count: Arc::clone(&drop_count),
                })),
            )
            .expect("refresh should start");

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let event = loop {
            if let Some(event) = worker.try_recv_event() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "refresh should finish"
            );
            thread::yield_now();
        };

        assert!(matches!(event, ModelProviderRefreshEvent::Finished { .. }));
        assert_eq!(drop_count.load(Ordering::SeqCst), 1);
    }

    fn request(provider_id: &str) -> ProviderSyncRequest {
        ProviderSyncRequest {
            provider_id: provider_id.to_string(),
        }
    }

    fn lease(client: Arc<dyn ProviderClient>) -> ProviderClientLease {
        ProviderClientLease::new(
            "fixture",
            runtime_domain::provider::ProviderKind::OpenAiCompatible,
            client,
            crate::ProviderPromptCachePolicy::Disabled,
        )
    }

    struct SuccessProvider {
        model_ids: Vec<String>,
    }

    impl ProviderClient for SuccessProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("model listing test must not stream") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async move {
                Ok(self
                    .model_ids
                    .iter()
                    .cloned()
                    .map(ModelDescriptor::new)
                    .collect())
            })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct FailingProvider;

    impl ProviderClient for FailingProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("model listing test must not stream") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async {
                Err(ProviderError::Transport(
                    "https://private.invalid/model-list-instruction-sentinel".to_string(),
                ))
            })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct PendingProvider;

    impl ProviderClient for PendingProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("model listing test must not stream") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(future::pending())
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct BlockingProvider {
        started: Mutex<Option<mpsc::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }

    impl ProviderClient for BlockingProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("model listing test must not stream") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async move {
                if let Some(started) = self
                    .started
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ = started.send(());
                }
                self.release
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                    .map_err(|_| ProviderError::Transport("fixture release closed".to_string()))?;
                Ok(Vec::new())
            })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }

    struct DropTrackedProvider {
        drop_count: Arc<AtomicUsize>,
    }

    impl Drop for DropTrackedProvider {
        fn drop(&mut self) {
            self.drop_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl ProviderClient for DropTrackedProvider {
        fn stream_prompt<'a>(
            &'a self,
            _request: &'a PromptRequest,
            _sink: &'a mut (dyn StreamEventSink + Send),
        ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
            Box::pin(async { unreachable!("model listing test must not stream") })
        }

        fn list_models<'a>(
            &'a self,
        ) -> ProviderFuture<'a, Result<Vec<ModelDescriptor>, ProviderError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::chat_completions()
        }
    }
}
