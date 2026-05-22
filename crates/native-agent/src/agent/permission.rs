use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

use mo_core::session::{
    NativeAgentEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
    RuntimePermissionRequest,
};
use mo_tools::{
    ToolPermissionDecision, ToolPermissionFuture, ToolPermissionHandler, ToolPermissionRequest,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::llm::runtime_tool_activity_update_from_permission_request;

const NATIVE_PERMISSION_REQUEST_PREFIX: &str = "native-permission";
const ALLOW_ONCE_OPTION_ID: &str = "allow_once";
const REJECT_ONCE_OPTION_ID: &str = "reject_once";
const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";

type PermissionResponseSender = oneshot::Sender<Option<String>>;

/// `NativePermissionBroker` 保存 native tool Ask 请求与 TUI 响应之间的等待关系。
#[derive(Debug, Clone, Default)]
pub(crate) struct NativePermissionBroker {
    next_request_id: Arc<AtomicUsize>,
    pending: Arc<Mutex<HashMap<String, PermissionResponseSender>>>,
}

impl NativePermissionBroker {
    pub(crate) fn handler(
        &self,
        sender: mpsc::Sender<NativeAgentEvent>,
    ) -> NativeToolPermissionHandler {
        NativeToolPermissionHandler {
            broker: self.clone(),
            sender,
        }
    }

    pub(crate) fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let sender = self
            .pending
            .lock()
            .expect("native permission lock should not be poisoned")
            .remove(request_id)
            .ok_or_else(|| format!("Native permission request is not pending: {request_id}"))?;
        sender
            .send(option_id)
            .map_err(|_| format!("Native permission request is no longer waiting: {request_id}"))
    }

    pub(crate) fn cancel_all(&self) {
        let pending = std::mem::take(
            &mut *self
                .pending
                .lock()
                .expect("native permission lock should not be poisoned"),
        );
        for (_, sender) in pending {
            let _ = sender.send(None);
        }
    }

    fn next_request_id(&self) -> String {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{NATIVE_PERMISSION_REQUEST_PREFIX}-{id}")
    }

    fn register(&self, request_id: String, sender: PermissionResponseSender) {
        self.pending
            .lock()
            .expect("native permission lock should not be poisoned")
            .insert(request_id, sender);
    }

    fn remove(&self, request_id: &str) {
        self.pending
            .lock()
            .expect("native permission lock should not be poisoned")
            .remove(request_id);
    }
}

/// `NativeToolPermissionHandler` 在工具执行前把 Ask 请求转交给 TUI。
pub(crate) struct NativeToolPermissionHandler {
    broker: NativePermissionBroker,
    sender: mpsc::Sender<NativeAgentEvent>,
}

impl ToolPermissionHandler for NativeToolPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        Box::pin(async move {
            let request_id = self.broker.next_request_id();
            let (response_sender, response_receiver) = oneshot::channel();
            self.broker.register(request_id.clone(), response_sender);

            let runtime_request = native_runtime_permission_request(&request_id, &request);
            if self
                .sender
                .send(NativeAgentEvent::PermissionRequested {
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
                deny_permission(&request.definition.name, "request was rejected")
            }
        })
    }
}

fn native_runtime_permission_request(
    request_id: &str,
    request: &ToolPermissionRequest,
) -> RuntimePermissionRequest {
    let tool_activity = runtime_tool_activity_update_from_permission_request(request_id, request);
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

    use mo_tools::{ToolCall, ToolDefinition, ToolKind, ToolPermissionPolicy};

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

    async fn recv_event(receiver: &mpsc::Receiver<NativeAgentEvent>) -> NativeAgentEvent {
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
    async fn native_permission_handler_round_trips_allow_response() {
        let broker = NativePermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender));
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
            NativeAgentEvent::PermissionRequested { request } => {
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
                    "native permission requests should include a tool activity preview"
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

    #[tokio::test]
    async fn native_permission_handler_denies_reject_response_without_leaking_pending_request() {
        let broker = NativePermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender));
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
            NativeAgentEvent::PermissionRequested { request } => request.request_id,
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker
            .respond_permission(&request_id, Some(REJECT_ONCE_OPTION_ID.to_string()))
            .expect("pending request should accept reject response");

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Deny {
                message: "Tool permission denied: write request was rejected".to_string()
            }
        );
        assert!(
            broker.respond_permission(&request_id, None).is_err(),
            "completed native permission requests should be removed"
        );
    }

    #[tokio::test]
    async fn native_permission_cancel_all_denies_pending_request() {
        let broker = NativePermissionBroker::default();
        let (sender, receiver) = mpsc::channel();
        let handler = Arc::new(broker.handler(sender));
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
            NativeAgentEvent::PermissionRequested { request } => request.request_id,
            other => panic!("expected permission request event, got {other:?}"),
        };

        broker.cancel_all();

        assert_eq!(
            decision.await.expect("permission task should finish"),
            ToolPermissionDecision::Deny {
                message: "Tool permission denied: write request was rejected".to_string()
            }
        );
        assert!(
            broker.respond_permission(&request_id, None).is_err(),
            "cancelled native permission requests should be removed"
        );
    }
}
