use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use crate::runtime::request_policy::RuntimeRequestPolicy;
use tokio_util::sync::CancellationToken;

use crate::runtime::session::RuntimeTarget;

use super::{
    ChatPerformanceMetrics, NativeChatError, NativeChatProgress, NativeChatRequest,
    NativeChatResponse, send_chat_with_cancellation_and_token_progress,
};

/// `NativeChatRuntimeState` 管理内置 native chat 请求的后台 worker 与取消状态。
#[derive(Default)]
pub(crate) struct NativeChatRuntimeState {
    pub(crate) receiver: Option<Receiver<NativeChatEvent>>,
    pub(crate) cancellation: Option<CancellationToken>,
    pub(crate) target: Option<RuntimeTarget>,
}

impl NativeChatRuntimeState {
    pub(crate) fn start(
        &mut self,
        request: NativeChatRequest,
        request_policy: RuntimeRequestPolicy,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        let target =
            RuntimeTarget::native_chat(request.provider_id.clone(), request.model_id.clone());
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run_native_chat_worker(
                        request,
                        request_policy,
                        thread_cancellation,
                        sender,
                    ));
                }
                Err(error) => {
                    let _ = sender.send(NativeChatEvent::Failed {
                        message: format!("start chat runtime: {error}"),
                    });
                }
            }
        });
        self.receiver = Some(receiver);
        self.cancellation = Some(cancellation);
        self.target = Some(target);
    }

    pub(crate) fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub(crate) fn reset_after_clear(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.receiver = None;
        self.target = None;
    }

    pub(crate) fn interrupt(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        self.receiver = None;
        self.target = None;
        true
    }

    pub(crate) fn current_target(&self) -> Option<&RuntimeTarget> {
        self.target.as_ref()
    }

    pub(crate) fn try_recv_event(&mut self) -> Option<NativeChatEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                if event.is_terminal() {
                    self.receiver = None;
                    self.cancellation = None;
                    self.target = None;
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.cancellation = None;
                self.target = None;
                Some(NativeChatEvent::Failed {
                    message: "chat request stopped before completion".to_string(),
                })
            }
        }
    }
}

/// `NativeChatEvent` 是 native chat worker 返回给 TUI runner 的事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NativeChatEvent {
    Retrying {
        message: String,
    },
    OutputTokenEstimate {
        total_tokens: usize,
    },
    Thinking {
        is_thinking: bool,
    },
    Finished {
        response: NativeChatResponse,
        metrics: Option<ChatPerformanceMetrics>,
    },
    Failed {
        message: String,
    },
    Interrupted,
}

impl NativeChatEvent {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished { .. } | Self::Failed { .. } | Self::Interrupted
        )
    }
}

async fn run_native_chat_worker(
    request: NativeChatRequest,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    sender: mpsc::Sender<NativeChatEvent>,
) {
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let attempt_result = tokio::time::timeout(
            request_policy.timeout(),
            send_chat_with_cancellation_and_token_progress(
                &request,
                &cancellation,
                move |progress| {
                    let event = match progress {
                        NativeChatProgress::OutputTokens { total_tokens } => {
                            NativeChatEvent::OutputTokenEstimate { total_tokens }
                        }
                        NativeChatProgress::Thinking { is_thinking } => {
                            NativeChatEvent::Thinking { is_thinking }
                        }
                    };
                    let _ = progress_sender.send(event);
                },
            ),
        )
        .await;

        match attempt_result {
            Err(_elapsed) if attempt < request_policy.attempts() => {
                if retry_native_chat_after_attempt(attempt, &request_policy, &cancellation, &sender)
                    .await
                {
                    return;
                }
            }
            Err(_elapsed) => {
                let _ = sender.send(NativeChatEvent::Failed {
                    message: format!(
                        "Chat request timed out after {}s",
                        request_policy.timeout().as_secs()
                    ),
                });
                return;
            }
            Ok(Ok(completion)) => {
                let _ = sender.send(NativeChatEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                });
                return;
            }
            Ok(Err(NativeChatError::Cancelled)) => {
                let _ = sender.send(NativeChatEvent::Interrupted);
                return;
            }
            Ok(Err(_error)) if attempt < request_policy.attempts() => {
                if retry_native_chat_after_attempt(attempt, &request_policy, &cancellation, &sender)
                    .await
                {
                    return;
                }
            }
            Ok(Err(error)) => {
                let _ = sender.send(NativeChatEvent::Failed {
                    message: error.to_string(),
                });
                return;
            }
        }
    }
}

async fn retry_native_chat_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<NativeChatEvent>,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(NativeChatEvent::Retrying {
        message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
    });
    tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = sender.send(NativeChatEvent::Interrupted);
            true
        }
        _ = tokio::time::sleep(request_policy.delay_for_retry(retry)) => false,
    }
}
