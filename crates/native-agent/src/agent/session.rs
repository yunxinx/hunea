use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use tokio_util::sync::CancellationToken;

use mo_core::{
    request_policy::RuntimeRequestPolicy,
    session::{NativeAgentEvent, RuntimeTarget},
};
use mo_tools::{SharedToolPermissionHandler, ToolExecutorRegistry};

use super::{
    NativeAgentError, NativeAgentRequest, NativePermissionBroker, response::NativeAgentProgress,
    turn::send_agent_loop_with_cancellation_and_progress,
};

/// `NativeAgentRuntimeState` 管理内置 native agent 请求的后台 worker 与取消状态。
#[derive(Default)]
pub struct NativeAgentRuntimeState {
    pub receiver: Option<Receiver<NativeAgentEvent>>,
    pub cancellation: Option<CancellationToken>,
    pub target: Option<RuntimeTarget>,
    permission_broker: Option<NativePermissionBroker>,
}

impl NativeAgentRuntimeState {
    pub fn start(
        &mut self,
        request: NativeAgentRequest,
        executor: ToolExecutorRegistry,
        request_policy: RuntimeRequestPolicy,
    ) {
        let (sender, receiver) = mpsc::channel();
        let cancellation = CancellationToken::default();
        let thread_cancellation = cancellation.clone();
        let target = request.target();
        let permission_broker = NativePermissionBroker::default();
        let thread_permission_broker = permission_broker.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            match runtime {
                Ok(runtime) => {
                    runtime.block_on(run_native_agent_worker(
                        request,
                        executor,
                        request_policy,
                        thread_cancellation,
                        thread_permission_broker,
                        sender,
                    ));
                }
                Err(error) => {
                    let _ = sender.send(NativeAgentEvent::Failed {
                        message: format!("start agent runtime: {error}"),
                    });
                }
            }
        });
        self.receiver = Some(receiver);
        self.cancellation = Some(cancellation);
        self.target = Some(target);
        self.permission_broker = Some(permission_broker);
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn reset_after_clear(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
        }
        self.receiver = None;
        self.target = None;
    }

    pub fn interrupt(&mut self) -> bool {
        if !self.is_running() {
            return false;
        }
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.cancel();
        }
        if let Some(permission_broker) = self.permission_broker.take() {
            permission_broker.cancel_all();
        }
        self.receiver = None;
        self.target = None;
        true
    }

    pub fn respond_permission(
        &mut self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let Some(permission_broker) = self.permission_broker.as_ref() else {
            return Err("Native agent is not waiting for permission".to_string());
        };
        permission_broker.respond_permission(request_id, option_id)
    }

    pub fn current_target(&self) -> Option<&RuntimeTarget> {
        self.target.as_ref()
    }

    pub fn try_recv_event(&mut self) -> Option<NativeAgentEvent> {
        let receiver = self.receiver.as_ref()?;
        match receiver.try_recv() {
            Ok(event) => {
                if event.is_terminal() {
                    self.receiver = None;
                    self.cancellation = None;
                    self.target = None;
                    if let Some(permission_broker) = self.permission_broker.take() {
                        permission_broker.cancel_all();
                    }
                }
                Some(event)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.receiver = None;
                self.cancellation = None;
                self.target = None;
                if let Some(permission_broker) = self.permission_broker.take() {
                    permission_broker.cancel_all();
                }
                Some(NativeAgentEvent::Failed {
                    message: "agent request stopped before completion".to_string(),
                })
            }
        }
    }
}

pub(crate) async fn run_native_agent_worker(
    request: NativeAgentRequest,
    executor: ToolExecutorRegistry,
    request_policy: RuntimeRequestPolicy,
    cancellation: CancellationToken,
    permission_broker: NativePermissionBroker,
    sender: mpsc::Sender<NativeAgentEvent>,
) {
    for attempt in 0..=request_policy.attempts() {
        let progress_sender = sender.clone();
        let permission_handler: SharedToolPermissionHandler =
            std::sync::Arc::new(permission_broker.handler(sender.clone()));
        let attempt_result = tokio::time::timeout(
            request_policy.timeout(),
            send_agent_loop_with_cancellation_and_progress(
                &request,
                executor.clone(),
                &cancellation,
                request_policy.tool_max_turns(),
                Some(permission_handler),
                move |progress| {
                    let event = native_agent_event_from_progress(progress);
                    let _ = progress_sender.send(event);
                },
            ),
        )
        .await;

        match attempt_result {
            Err(_elapsed) if attempt < request_policy.attempts() => {
                permission_broker.cancel_all();
                if retry_native_agent_after_attempt(
                    attempt,
                    &request_policy,
                    &cancellation,
                    &sender,
                )
                .await
                {
                    return;
                }
            }
            Err(_elapsed) => {
                permission_broker.cancel_all();
                let _ = sender.send(NativeAgentEvent::Failed {
                    message: format!(
                        "Agent request timed out after {}s",
                        request_policy.timeout().as_secs()
                    ),
                });
                return;
            }
            Ok(Ok(completion)) => {
                permission_broker.cancel_all();
                let _ = sender.send(NativeAgentEvent::Finished {
                    response: completion.response,
                    metrics: completion.metrics,
                });
                return;
            }
            Ok(Err(NativeAgentError::Cancelled)) => {
                permission_broker.cancel_all();
                let _ = sender.send(NativeAgentEvent::Interrupted);
                return;
            }
            Ok(Err(_error)) if attempt < request_policy.attempts() => {
                permission_broker.cancel_all();
                if retry_native_agent_after_attempt(
                    attempt,
                    &request_policy,
                    &cancellation,
                    &sender,
                )
                .await
                {
                    return;
                }
            }
            Ok(Err(error)) => {
                permission_broker.cancel_all();
                let _ = sender.send(NativeAgentEvent::Failed {
                    message: error.to_string(),
                });
                return;
            }
        }
    }
}

fn native_agent_event_from_progress(progress: NativeAgentProgress) -> NativeAgentEvent {
    match progress {
        NativeAgentProgress::OutputTokens { total_tokens } => {
            NativeAgentEvent::OutputTokenEstimate { total_tokens }
        }
        NativeAgentProgress::InputTokens { total_tokens } => {
            NativeAgentEvent::InputTokenEstimate { total_tokens }
        }
        NativeAgentProgress::Thinking { is_thinking } => NativeAgentEvent::Thinking { is_thinking },
        NativeAgentProgress::AssistantDelta { content } => {
            NativeAgentEvent::AssistantDelta { content }
        }
        NativeAgentProgress::ReasoningDelta { content } => {
            NativeAgentEvent::ReasoningDelta { content }
        }
        NativeAgentProgress::ToolActivityStarted { activity } => {
            NativeAgentEvent::ToolActivityStarted { activity }
        }
        NativeAgentProgress::ToolActivityUpdated { update } => {
            NativeAgentEvent::ToolActivityUpdated { update }
        }
    }
}

async fn retry_native_agent_after_attempt(
    attempt: usize,
    request_policy: &RuntimeRequestPolicy,
    cancellation: &CancellationToken,
    sender: &mpsc::Sender<NativeAgentEvent>,
) -> bool {
    let retry = attempt + 1;
    let _ = sender.send(NativeAgentEvent::Retrying {
        message: format!("Reconnecting... {retry}/{}", request_policy.attempts()),
    });
    tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = sender.send(NativeAgentEvent::Interrupted);
            true
        }
        _ = tokio::time::sleep(request_policy.delay_for_retry(retry)) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crate::{ChatMessage, NativeAgentRequest, NativeAgentResponse, ProviderKind};
    use mo_core::{request_policy::RuntimeRequestPolicy, session::RuntimeTarget};
    use mo_tools::ToolExecutorRegistry;
    use tokio_util::sync::CancellationToken;

    use super::{NativeAgentEvent, NativeAgentRuntimeState, NativePermissionBroker};

    #[test]
    fn native_agent_runtime_clears_receiver_after_terminal_event() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(NativeAgentEvent::Interrupted)
            .expect("send terminal event");
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
        };

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Interrupted)
        );
        assert!(!runtime.is_running());
        assert!(runtime.current_target().is_none());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_retry_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
        };

        sender
            .send(NativeAgentEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
            .expect("retry event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Retrying {
                message: "Reconnecting... 1/3".to_string(),
            })
        );
        assert!(runtime.is_running());

        sender
            .send(NativeAgentEvent::Finished {
                response: NativeAgentResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
            .expect("finish event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::Finished {
                response: NativeAgentResponse {
                    content: "完成".to_string(),
                    reasoning_content: None,
                    reasoning_duration: None,
                },
                metrics: None,
            })
        );
        assert!(!runtime.is_running());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_token_estimate_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
        };

        sender
            .send(NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 })
            .expect("token estimate event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::OutputTokenEstimate { total_tokens: 12 })
        );
        assert!(runtime.is_running());
    }

    #[test]
    fn native_agent_runtime_keeps_receiver_after_text_delta_event() {
        let (sender, receiver) = mpsc::channel();
        let mut runtime = NativeAgentRuntimeState {
            receiver: Some(receiver),
            cancellation: Some(CancellationToken::new()),
            target: Some(RuntimeTarget::native_agent("provider", "model")),
            permission_broker: None,
        };

        sender
            .send(NativeAgentEvent::AssistantDelta {
                content: "partial".to_string(),
            })
            .expect("assistant delta event should be queued");

        assert_eq!(
            runtime.try_recv_event(),
            Some(NativeAgentEvent::AssistantDelta {
                content: "partial".to_string(),
            })
        );
        assert!(runtime.is_running());
    }

    #[tokio::test]
    async fn native_agent_worker_reports_interrupted_when_pre_cancelled() {
        let request = NativeAgentRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("hello".to_string())],
        );
        let executor = ToolExecutorRegistry::new();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let (sender, receiver) = mpsc::channel();

        super::run_native_agent_worker(
            request,
            executor,
            RuntimeRequestPolicy::default(),
            cancellation,
            NativePermissionBroker::default(),
            sender,
        )
        .await;

        assert_eq!(
            receiver.recv().expect("worker should emit an event"),
            NativeAgentEvent::Interrupted
        );
    }
}
