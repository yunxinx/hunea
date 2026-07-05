struct WriteLikeTool;

impl Tool for WriteLikeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("write")
            .with_label("Write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            let new_text = call
                .arguments
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("written")
                .to_string();
            ToolResult::success(call.call_id, "written").with_details(serde_json::json!({
                "path": "temp.md",
                "new_text": new_text,
            }))
        })
    }
}

struct AskWriteLikeTool;

impl Tool for AskWriteLikeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("write")
            .with_label("Write")
            .with_kind(ToolKind::Write)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            let new_text = call
                .arguments
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("written")
                .to_string();
            ToolResult::success(call.call_id, "written").with_details(serde_json::json!({
                "path": "temp.md",
                "new_text": new_text,
            }))
        })
    }

    fn permission_preview(
        &self,
        call: &RuntimeToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        Some(ToolPermissionPreview {
            path: "temp.md".to_string(),
            old_text: None,
            new_text: call
                .arguments
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            is_truncated: false,
            snapshot: None,
        })
    }
}

struct ConditionalTerminatingTool;

impl Tool for ConditionalTerminatingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            let should_terminate = call
                .arguments
                .get("terminate")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let result = ToolResult::success(call.call_id.clone(), call.call_id);
            if should_terminate {
                result.with_terminate()
            } else {
                result
            }
        })
    }
}

struct TerminatingTool;

impl Tool for TerminatingTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            ToolResult::success(call.call_id, "terminate here").with_terminate()
        })
    }
}

struct EchoTool;

impl Tool for EchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
    }
}

struct ClockAdvanceTool {
    clock: ManualClock,
}

impl Tool for ClockAdvanceTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            self.clock.advance(Duration::from_millis(100));
            ToolResult::success(call.call_id, "echoed")
        })
    }
}

struct LargeOutputTool;

impl Tool for LargeOutputTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "tool output ".repeat(80)) })
    }
}

struct AskEchoTool;

impl Tool for AskEchoTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Other)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
    }
}

struct SleepyAllowPermissionHandler;

impl ToolPermissionHandler for SleepyAllowPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        _request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ToolPermissionDecision::Allow
        })
    }
}

struct CapturingAllowPermissionHandler {
    preview: Arc<Mutex<Option<ToolPermissionPreview>>>,
}

impl ToolPermissionHandler for CapturingAllowPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        *self.preview.lock().expect("preview lock should not poison") = request.preview;
        Box::pin(async { ToolPermissionDecision::Allow })
    }
}

struct BlockingPreviewProbePermissionHandler {
    preview: Arc<Mutex<Option<ToolPermissionPreview>>>,
    timer_fired: Arc<AtomicBool>,
    timer_fired_before_permission: Arc<AtomicBool>,
}

impl ToolPermissionHandler for BlockingPreviewProbePermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        *self.preview.lock().expect("preview lock should not poison") = request.preview;
        self.timer_fired_before_permission
            .store(self.timer_fired.load(Ordering::SeqCst), Ordering::SeqCst);
        Box::pin(async { ToolPermissionDecision::Allow })
    }
}

struct PermissionEventCountProbe {
    events: Arc<Mutex<Vec<ToolLoopProgress>>>,
    event_count_at_permission: Arc<Mutex<Option<usize>>>,
}

impl ToolPermissionHandler for PermissionEventCountProbe {
    fn request_permission<'a>(
        &'a self,
        _request: ToolPermissionRequest,
        _cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        let event_count = self
            .events
            .lock()
            .expect("events lock should not poison")
            .len();
        *self
            .event_count_at_permission
            .lock()
            .expect("event count lock should not poison") = Some(event_count);
        Box::pin(async { ToolPermissionDecision::Allow })
    }
}

struct AskPreviewTool;

impl Tool for AskPreviewTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Edit)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
    }

    fn permission_preview(
        &self,
        _call: &RuntimeToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        Some(ToolPermissionPreview {
            path: "temp.md".to_string(),
            old_text: Some("old\n".to_string()),
            new_text: "new\n".to_string(),
            is_truncated: false,
            snapshot: None,
        })
    }
}

struct SlowPreviewTool;

impl Tool for SlowPreviewTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Echo")
            .with_kind(ToolKind::Edit)
            .with_permission_policy(ToolPermissionPolicy::Ask)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "echoed") })
    }

    fn permission_preview(
        &self,
        _call: &RuntimeToolCall,
        _cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        std::thread::sleep(Duration::from_millis(500));
        Some(ToolPermissionPreview {
            path: "temp.md".to_string(),
            old_text: Some("old\n".to_string()),
            new_text: "new\n".to_string(),
            is_truncated: false,
            snapshot: None,
        })
    }
}

struct FailingExecuteTool;

impl Tool for FailingExecuteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("echo")
            .with_label("Shell:")
            .with_kind(ToolKind::Execute)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move {
            ToolResult::error(call.call_id, "before failure\n\nCommand exited with code 7")
                .with_details(serde_json::json!({
                "execution_kind": "command",
                "exit_code": 7,
                "duration_ms": 250,
                "timed_out": false,
                "cancelled": false
            }))
                .with_display_content("before failure")
        })
    }
}

struct TerminalProgressTool;

impl Tool for TerminalProgressTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("run")
            .with_label("Run")
            .with_kind(ToolKind::Execute)
            .with_permission_policy(ToolPermissionPolicy::Always)
    }

    fn execute<'a>(
        &'a self,
        call: RuntimeToolCall,
        _cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        Box::pin(async move { ToolResult::success(call.call_id, "done") })
    }

    fn execute_with_context<'a>(
        &'a self,
        call: RuntimeToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        context.emit(ToolProgress::TerminalUpdated {
            snapshot: ToolTerminalSnapshot {
                terminal_id: call.call_id.clone(),
                command: Some("cargo check".to_string()),
                cwd: Some("/workspace".to_string()),
                output: "Checking hunea".to_string(),
                truncated: false,
                exit_status: None,
                released: false,
            },
        });
        Box::pin(async move { ToolResult::success(call.call_id, "done") })
    }
}
