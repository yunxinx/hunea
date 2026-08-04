use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use provider_protocol::{
    ConversationItem, FinishReason, ModelDescriptor, PromptCompletion, PromptRequest,
    ProviderCapabilities, ProviderClient, ProviderError, ProviderFuture, Role, StreamEvent,
    StreamEventSink,
};
use tokio_util::sync::CancellationToken;
use tool_loop_runtime::{ToolLoopOptions, run_tool_loop};
use tool_runtime::{
    Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture,
    ToolExecutorRegistry, ToolKind, ToolPermissionFileSnapshot, ToolPermissionPolicy,
    ToolPermissionPreview, ToolResult,
};

use super::*;

struct SingleWriteThenTextProvider {
    calls: Mutex<usize>,
    path: String,
}

impl SingleWriteThenTextProvider {
    fn new(path: &str) -> Self {
        Self {
            calls: Mutex::new(0),
            path: path.to_string(),
        }
    }
}

impl ProviderClient for SingleWriteThenTextProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let call_number = {
                let mut calls = self.calls.lock().expect("provider lock should not poison");
                *calls += 1;
                *calls
            };
            sink.emit(StreamEvent::TurnStarted);
            let response = if call_number == 1 {
                let call = provider_protocol::ToolCall::new(
                    "write-call",
                    "write",
                    serde_json::json!({
                        "path": self.path,
                        "content": "body",
                    })
                    .to_string(),
                );
                PromptCompletion::new(
                    vec![ConversationItem::assistant_with_tool_calls(
                        String::new(),
                        vec![call],
                    )],
                    FinishReason::ToolCalls,
                    None,
                )
            } else {
                PromptCompletion::new(
                    vec![ConversationItem::text(Role::Assistant, "done")],
                    FinishReason::Stop,
                    None,
                )
            };
            sink.emit(StreamEvent::TurnCompleted(response.clone()));
            Ok(response)
        })
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

struct PreviewTrackingWriteTool {
    preview_calls: Arc<AtomicUsize>,
    execution_snapshots: Arc<Mutex<Vec<Option<ToolPermissionFileSnapshot>>>>,
}

impl Tool for PreviewTrackingWriteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "written") })
    }

    fn execute_with_context<'a>(
        &'a self,
        call: RuntimeToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        self.execution_snapshots
            .lock()
            .expect("snapshot lock should not poison")
            .push(context.permission_snapshot().cloned());
        Box::pin(async move { ToolResult::success(call.call_id, "written") })
    }

    fn permission_preview(
        &self,
        call: &RuntimeToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        self.preview_calls.fetch_add(1, Ordering::SeqCst);
        let path = call
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .expect("write call should contain a path")
            .to_string();
        Some(ToolPermissionPreview {
            path,
            old_text: None,
            new_text: "body".to_string(),
            is_truncated: false,
            snapshot: Some(ToolPermissionFileSnapshot {
                content_hash: 7,
                byte_len: 4,
                modified_at: None,
            }),
        })
    }
}

fn prompt_request() -> PromptRequest {
    PromptRequest::new(
        "test-model",
        vec![ConversationItem::text(Role::User, "write a file")],
    )
}

#[tokio::test]
async fn session_allow_keeps_preview_and_snapshot_checks_in_the_tool_loop() {
    let preview_calls = Arc::new(AtomicUsize::new(0));
    let execution_snapshots = Arc::new(Mutex::new(Vec::new()));
    let mut executor = ToolExecutorRegistry::new();
    executor.insert(PreviewTrackingWriteTool {
        preview_calls: Arc::clone(&preview_calls),
        execution_snapshots: Arc::clone(&execution_snapshots),
    });

    let broker = ConversationPermissionBroker::default();
    let (sender, receiver) = mpsc::channel();
    let handler = Arc::new(broker.handler(sender));
    let first_provider = Arc::new(SingleWriteThenTextProvider::new("src/main.rs"));
    let first_provider_task = Arc::clone(&first_provider);
    let first_handler = Arc::clone(&handler);
    let first_cancellation = CancellationToken::new();
    let first_executor = executor.clone();
    let first = tokio::spawn(async move {
        run_tool_loop(
            &*first_provider_task,
            prompt_request(),
            first_executor,
            &first_cancellation,
            ToolLoopOptions {
                permission_handler: Some(first_handler),
                ..ToolLoopOptions::default()
            },
            |_| {},
        )
        .await
    });

    let request_id = match super::recv_event(&receiver).await {
        ConversationEvent::PermissionRequested { request } => {
            assert_eq!(request.options.len(), 4);
            request.request_id
        }
        other => panic!("expected permission request event, got {other:?}"),
    };
    broker
        .respond_permission(&request_id, Some(ALLOW_ALWAYS_OPTION_ID.to_string()))
        .expect("first request should accept a session rule");
    first
        .await
        .expect("first tool loop should finish")
        .expect("first tool loop should succeed");

    let second_provider = SingleWriteThenTextProvider::new("src/main.rs");
    run_tool_loop(
        &second_provider,
        prompt_request(),
        executor,
        &CancellationToken::new(),
        ToolLoopOptions {
            permission_handler: Some(handler),
            ..ToolLoopOptions::default()
        },
        |_| {},
    )
    .await
    .expect("matching session rule should allow the second tool loop");

    assert_eq!(preview_calls.load(Ordering::SeqCst), 2);
    let execution_snapshots = execution_snapshots
        .lock()
        .expect("snapshot lock should not poison");
    assert_eq!(execution_snapshots.len(), 2);
    assert!(execution_snapshots.iter().all(|snapshot| {
        snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.content_hash == 7 && snapshot.byte_len == 4)
    }));
    match receiver.try_recv() {
        Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => {}
        Ok(event) => panic!("matching session rule emitted an event: {event:?}"),
    }
}
