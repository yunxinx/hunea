use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

use runtime_domain::session::{
    ConversationEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
    RuntimePermissionRequest,
};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ToolPermissionDecision, ToolPermissionFuture, ToolPermissionHandler, ToolPermissionRequest,
};

use tool_loop_runtime::runtime_tool_activity_update_from_permission_request;

const CONVERSATION_PERMISSION_REQUEST_PREFIX: &str = "conversation-permission";
const ALLOW_ONCE_OPTION_ID: &str = "allow_once";
const REJECT_ONCE_OPTION_ID: &str = "reject_once";
const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";

type PermissionResponseSender = oneshot::Sender<Option<String>>;

/// `ConversationTimeoutPause` 在用户审批期间暂停 request timeout 计时。
#[derive(Debug, Clone)]
pub(crate) struct ConversationTimeoutPause {
    sender: watch::Sender<usize>,
}

impl Default for ConversationTimeoutPause {
    fn default() -> Self {
        let (sender, _) = watch::channel(0);
        Self { sender }
    }
}

impl ConversationTimeoutPause {
    pub(crate) fn pause(&self) -> ConversationTimeoutPauseGuard {
        let count = self.current_count().saturating_add(1);
        let _ = self.sender.send(count);
        ConversationTimeoutPauseGuard {
            pause: self.clone(),
        }
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<usize> {
        self.sender.subscribe()
    }

    fn resume(&self) {
        let count = self.current_count().saturating_sub(1);
        let _ = self.sender.send(count);
    }

    fn current_count(&self) -> usize {
        *self.sender.borrow()
    }
}

/// `ConversationTimeoutPauseGuard` 在 drop 时恢复 request timeout 计时。
#[derive(Debug)]
pub(crate) struct ConversationTimeoutPauseGuard {
    pause: ConversationTimeoutPause,
}

impl Drop for ConversationTimeoutPauseGuard {
    fn drop(&mut self) {
        self.pause.resume();
    }
}

/// `ConversationPermissionBroker` 保存 conversation tool Ask 请求与 TUI 响应之间的等待关系。
#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationPermissionBroker {
    next_request_id: Arc<AtomicUsize>,
    pending: Arc<Mutex<HashMap<String, PermissionResponseSender>>>,
}

impl ConversationPermissionBroker {
    pub(crate) fn handler(
        &self,
        sender: mpsc::Sender<ConversationEvent>,
        timeout_pause: ConversationTimeoutPause,
    ) -> ConversationToolPermissionHandler {
        ConversationToolPermissionHandler {
            broker: self.clone(),
            sender,
            timeout_pause,
        }
    }

    pub(crate) fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let sender = self.pending_guard().remove(request_id).ok_or_else(|| {
            format!("Conversation permission request is not pending: {request_id}")
        })?;
        sender.send(option_id).map_err(|_| {
            format!("Conversation permission request is no longer waiting: {request_id}")
        })
    }

    pub(crate) fn cancel_all(&self) {
        let pending = std::mem::take(&mut *self.pending_guard());
        for (_, sender) in pending {
            let _ = sender.send(None);
        }
    }

    fn next_request_id(&self) -> String {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{CONVERSATION_PERMISSION_REQUEST_PREFIX}-{id}")
    }

    fn register(&self, request_id: String, sender: PermissionResponseSender) {
        self.pending_guard().insert(request_id, sender);
    }

    fn remove(&self, request_id: &str) {
        self.pending_guard().remove(request_id);
    }

    fn pending_guard(&self) -> MutexGuard<'_, HashMap<String, PermissionResponseSender>> {
        match self.pending.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// `ConversationToolPermissionHandler` 在工具执行前把 Ask 请求转交给 TUI。
pub(crate) struct ConversationToolPermissionHandler {
    broker: ConversationPermissionBroker,
    sender: mpsc::Sender<ConversationEvent>,
    timeout_pause: ConversationTimeoutPause,
}

impl ToolPermissionHandler for ConversationToolPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        Box::pin(async move {
            let _timeout_pause = self.timeout_pause.pause();
            let request_id = self.broker.next_request_id();
            let (response_sender, response_receiver) = oneshot::channel();
            self.broker.register(request_id.clone(), response_sender);

            let runtime_request = conversation_runtime_permission_request(&request_id, &request);
            if self
                .sender
                .send(ConversationEvent::PermissionRequested {
                    request: runtime_request,
                })
                .is_err()
            {
                self.broker.remove(&request_id);
                return deny_permission(&request.definition.name, "runtime is unavailable");
            }

            let option_id = tokio::select! {
                _ = cancellation.cancelled() => {
                    self.broker.remove(&request_id);
                    None
                }
                response = response_receiver => response.ok().flatten(),
            };

            if option_id.as_deref() == Some(ALLOW_ONCE_OPTION_ID) {
                ToolPermissionDecision::Allow
            } else {
                deny_permission(&request.definition.name, "user rejected the tool call")
            }
        })
    }
}

fn conversation_runtime_permission_request(
    request_id: &str,
    request: &ToolPermissionRequest,
) -> RuntimePermissionRequest {
    let tool_activity =
        runtime_tool_activity_update_from_permission_request(&request.call.call_id, request);
    RuntimePermissionRequest::new(
        request_id.to_string(),
        tool_activity.title.clone(),
        vec![
            RuntimePermissionOption::new(
                ALLOW_ONCE_OPTION_ID,
                "Yes",
                RuntimePermissionOptionKind::AllowOnce,
            ),
            RuntimePermissionOption::new(
                REJECT_ONCE_OPTION_ID,
                "No",
                RuntimePermissionOptionKind::RejectOnce,
            ),
        ],
    )
    .with_tool_activity(tool_activity)
}

fn deny_permission(tool_name: &str, reason: &str) -> ToolPermissionDecision {
    ToolPermissionDecision::Deny {
        message: format!("{TOOL_PERMISSION_DENIED}: {tool_name} {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    use runtime_domain::session::RuntimeToolActivityContent;
    use tool_runtime::{
        ToolCall, ToolDefinition, ToolKind, ToolPermissionPolicy, ToolPermissionPreview,
    };

    use super::*;

    fn permission_request() -> ToolPermissionRequest {
        ToolPermissionRequest::new(
            ToolCall::new(
                "write",
                "write",
                serde_json::json!({
                    "path": "TEMP.md",
                    "content": "body",
                }),
            ),
            ToolDefinition::new("write")
                .with_label("Write")
                .with_kind(ToolKind::Write)
                .with_permission_policy(ToolPermissionPolicy::Ask),
        )
    }

    fn permission_request_with_preview() -> ToolPermissionRequest {
        permission_request().with_preview(ToolPermissionPreview {
            path: "TEMP.md".to_string(),
            old_text: Some("old\n".to_string()),
            new_text: "new\n".to_string(),
            is_truncated: false,
            snapshot: None,
        })
    }

    async fn recv_event(receiver: &mpsc::Receiver<ConversationEvent>) -> ConversationEvent {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match receiver.try_recv() {
                    Ok(event) => return event,
                    Err(mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        panic!("permission event sender disconnected")
                    }
                }
            }
        })
        .await
        .expect("permission event should be emitted")
    }

    #[tokio::test]
    async fn conversation_permission_handler_round_trips_allow_response() {
        let broker = ConversationPermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender, ConversationTimeoutPause::default()));
        let cancellation = CancellationToken::new();
        let task_handler = Arc::clone(&handler);
        let task_cancellation = cancellation.clone();

        let decision = tokio::spawn(async move {
            task_handler
                .request_permission(permission_request(), &task_cancellation)
                .await
        });

        let event = recv_event(&receiver).await;
        let request_id = match event {
            ConversationEvent::PermissionRequested { request } => {
                assert_eq!(request.title, Some("Write TEMP.md".to_string()));
                assert_eq!(
                    request.option_id_for(RuntimePermissionOptionKind::AllowOnce),
                    Some(ALLOW_ONCE_OPTION_ID.to_string())
                );
                assert_eq!(
                    request.option_id_for(RuntimePermissionOptionKind::RejectOnce),
                    Some(REJECT_ONCE_OPTION_ID.to_string())
                );
                assert!(
                    request.tool_activity.is_some(),
                    "conversation permission requests should include a tool activity preview"
                );
                assert_eq!(
                    request
                        .tool_activity
                        .as_ref()
                        .map(|activity| activity.activity_id.as_str()),
                    Some("write"),
                    "tool activity preview should keep the original provider tool call id"
                );
                request.request_id
            }
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker
            .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
            .expect("pending request should accept allow response");

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Allow
        );
    }

    #[test]
    fn conversation_permission_response_recovers_from_poisoned_pending_lock() {
        let broker = ConversationPermissionBroker::default();
        let request_id = broker.next_request_id();
        let (response_sender, mut response_receiver) = oneshot::channel();
        broker.register(request_id.clone(), response_sender);

        let poison_broker = broker.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poison_broker
                .pending
                .lock()
                .expect("test should acquire the pending lock before poisoning");
            panic!("poison pending lock");
        })
        .join();

        broker
            .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
            .expect("poisoned lock should not prevent responding to pending permission");
        assert_eq!(
            response_receiver
                .try_recv()
                .expect("permission response should be delivered"),
            Some(ALLOW_ONCE_OPTION_ID.to_string())
        );
    }

    #[tokio::test]
    async fn conversation_permission_request_preserves_tool_diff_preview() {
        let broker = ConversationPermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender, ConversationTimeoutPause::default()));
        let cancellation = CancellationToken::new();
        let task_handler = Arc::clone(&handler);
        let task_cancellation = cancellation.clone();

        let decision = tokio::spawn(async move {
            task_handler
                .request_permission(permission_request_with_preview(), &task_cancellation)
                .await
        });

        let event = recv_event(&receiver).await;
        let request_id = match event {
            ConversationEvent::PermissionRequested { request } => {
                assert!(matches!(
                    request
                        .tool_activity
                        .as_ref()
                        .and_then(|activity| activity.content.as_ref())
                        .and_then(|content| content.first()),
                    Some(RuntimeToolActivityContent::Diff {
                        path,
                        old_text,
                        new_text,
                        ..
                    }) if path == "TEMP.md"
                        && old_text.as_deref() == Some("old\n")
                        && new_text == "new\n"
                ));
                request.request_id
            }
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker
            .respond_permission(&request_id, Some(ALLOW_ONCE_OPTION_ID.to_string()))
            .expect("pending request should accept allow response");

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Allow
        );
    }

    #[tokio::test]
    async fn conversation_permission_handler_denies_reject_response_without_leaking_pending_request()
     {
        let broker = ConversationPermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender, ConversationTimeoutPause::default()));
        let cancellation = CancellationToken::new();
        let task_handler = Arc::clone(&handler);
        let task_cancellation = cancellation.clone();

        let decision = tokio::spawn(async move {
            task_handler
                .request_permission(permission_request(), &task_cancellation)
                .await
        });

        let event = recv_event(&receiver).await;
        let request_id = match event {
            ConversationEvent::PermissionRequested { request } => request.request_id,
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker
            .respond_permission(&request_id, Some(REJECT_ONCE_OPTION_ID.to_string()))
            .expect("pending request should accept reject response");

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Deny {
                message: "Tool permission denied: write user rejected the tool call".to_string()
            }
        );
        assert!(
            broker.respond_permission(&request_id, None).is_err(),
            "completed conversation permission requests should be removed"
        );
    }

    #[tokio::test]
    async fn conversation_permission_cancel_all_denies_pending_request() {
        let broker = ConversationPermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender, ConversationTimeoutPause::default()));
        let cancellation = CancellationToken::new();
        let task_handler = Arc::clone(&handler);
        let task_cancellation = cancellation.clone();

        let decision = tokio::spawn(async move {
            task_handler
                .request_permission(permission_request(), &task_cancellation)
                .await
        });

        let event = recv_event(&receiver).await;
        let request_id = match event {
            ConversationEvent::PermissionRequested { request } => request.request_id,
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker.cancel_all();

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Deny {
                message: "Tool permission denied: write user rejected the tool call".to_string()
            }
        );
        assert!(
            broker.respond_permission(&request_id, None).is_err(),
            "cancelled conversation permission requests should be removed"
        );
    }
}
