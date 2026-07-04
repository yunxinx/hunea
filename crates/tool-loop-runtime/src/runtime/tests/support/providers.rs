struct FakeProvider {
    calls: Mutex<usize>,
}

impl ProviderClient for FakeProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let mut calls = self.calls.lock().expect("fake lock should not poison");
            *calls += 1;
            if *calls == 1 {
                let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                sink.emit(StreamEvent::TextDelta("checking".to_string()));
                let response = tool_call_completion("checking".to_string(), vec![call]);
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            } else {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            }
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

struct UsageProvider;

impl ProviderClient for UsageProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            sink.emit(StreamEvent::TurnStarted);
            sink.emit(StreamEvent::TextDelta("done".to_string()));
            sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                None,
                Some(3),
                Some(46),
            )));
            sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                None,
                Some(5),
                Some(48),
            )));
            let response = text_completion_with_usage(
                Role::Assistant,
                "done",
                TokenUsage::new(None, Some(5), Some(48)),
            );
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

struct DelayedFirstTokenProvider;

impl ProviderClient for DelayedFirstTokenProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            sink.emit(StreamEvent::TurnStarted);
            tokio::time::sleep(Duration::from_millis(40)).await;
            sink.emit(StreamEvent::TextDelta("done".to_string()));
            let response = text_completion(Role::Assistant, "done");
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

struct ToolLoopUsageProvider;

impl ProviderClient for ToolLoopUsageProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            sink.emit(StreamEvent::TurnStarted);
            if !has_tool_result {
                let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(20),
                    None,
                )));
                let response = tool_call_completion_with_usage(
                    "hello world".to_string(),
                    vec![call],
                    TokenUsage::new(None, Some(20), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            } else {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(5),
                    None,
                )));
                let response = text_completion_with_usage(
                    Role::Assistant,
                    "done",
                    TokenUsage::new(None, Some(5), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            }
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

struct MixedUsageProvider;

impl ProviderClient for MixedUsageProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            sink.emit(StreamEvent::TurnStarted);
            if !has_tool_result {
                let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(20),
                    None,
                )));
                let response = tool_call_completion_with_usage(
                    "hello world".to_string(),
                    vec![call],
                    TokenUsage::new(None, Some(20), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            } else {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            }
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

#[derive(Clone)]
struct ManualClock {
    started_at: Instant,
    elapsed_ms: Arc<AtomicU64>,
}

impl ManualClock {
    fn new() -> Self {
        Self {
            started_at: Instant::now(),
            elapsed_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    fn now(&self) -> Instant {
        self.started_at + Duration::from_millis(self.elapsed_ms.load(Ordering::SeqCst))
    }

    fn advance(&self, duration: Duration) {
        let elapsed_ms = duration.as_millis().min(u128::from(u64::MAX)) as u64;
        self.elapsed_ms.fetch_add(elapsed_ms, Ordering::SeqCst);
    }

    fn tool_loop_clock(&self) -> ToolLoopClock {
        let clock = self.clone();
        ToolLoopClock::new(move || clock.now())
    }
}

struct TimedToolLoopProvider {
    clock: ManualClock,
}

impl ProviderClient for TimedToolLoopProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            sink.emit(StreamEvent::TurnStarted);
            self.clock.advance(Duration::from_millis(10));
            if !has_tool_result {
                let call = ToolCall::new("call-1", "echo", r#"{"text":"hi"}"#);
                sink.emit(StreamEvent::TextDelta("hello world".to_string()));
                self.clock.advance(Duration::from_millis(30));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(20),
                    None,
                )));
                let response = tool_call_completion_with_usage(
                    "hello world".to_string(),
                    vec![call],
                    TokenUsage::new(None, Some(20), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            } else {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                self.clock.advance(Duration::from_millis(30));
                sink.emit(StreamEvent::UsageUpdated(TokenUsage::new(
                    None,
                    Some(5),
                    None,
                )));
                let response = text_completion_with_usage(
                    Role::Assistant,
                    "done",
                    TokenUsage::new(None, Some(5), None),
                );
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            }
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

struct ResponseOnlyTimedTextProvider {
    clock: ManualClock,
}

impl ProviderClient for ResponseOnlyTimedTextProvider {
    fn stream_prompt<'a>(
        &'a self,
        _request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            sink.emit(StreamEvent::TurnStarted);
            self.clock.advance(Duration::from_millis(100));
            let response = text_completion(Role::Assistant, "done");
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

struct MultiToolBatchProvider {
    calls: Mutex<usize>,
    request_items: Mutex<Vec<Vec<ConversationItem>>>,
}

impl MultiToolBatchProvider {
    fn new() -> Self {
        Self {
            calls: Mutex::new(0),
            request_items: Mutex::new(Vec::new()),
        }
    }
}

impl ProviderClient for MultiToolBatchProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            self.request_items
                .lock()
                .expect("request items lock should not poison")
                .push(request.items.clone());
            let call_count = {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
                *calls
            };
            sink.emit(StreamEvent::TurnStarted);
            if call_count == 1 {
                let terminating_call = ToolCall::new(
                    "call-terminate",
                    "echo",
                    r#"{"terminate":true}"#.to_string(),
                );
                let continuing_call = ToolCall::new(
                    "call-continue",
                    "echo",
                    r#"{"terminate":false}"#.to_string(),
                );
                let calls = vec![terminating_call, continuing_call];
                sink.emit(StreamEvent::TextDelta("checking".to_string()));
                let response = tool_call_completion("checking".to_string(), calls);
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            } else {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                Ok(response)
            }
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

struct WriteArgumentStreamingProvider {
    calls: Mutex<usize>,
}

struct WriteArgumentCompletedProvider {
    calls: Mutex<usize>,
}

struct WriteArgumentResponseOnlyProvider {
    calls: Mutex<usize>,
}

fn write_arguments_for_token_tests() -> String {
    serde_json::json!({
        "path": "temp.md",
        "content": "generated write content ".repeat(80),
    })
    .to_string()
}

impl ProviderClient for WriteArgumentStreamingProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
            }
            sink.emit(StreamEvent::TurnStarted);
            if has_tool_result {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                return Ok(response);
            }

            let arguments = write_arguments_for_token_tests();
            let arguments_value: serde_json::Value =
                serde_json::from_str(&arguments).expect("valid JSON");
            let content = arguments_value
                .get("content")
                .and_then(serde_json::Value::as_str)
                .expect("write test arguments should contain content")
                .to_string();
            let call = ToolCall::new("call-write", "write", arguments);
            sink.emit(StreamEvent::ToolCallStarted {
                index: 0,
                call_id: "call-write".to_string(),
                name: "write".to_string(),
            });
            sink.emit(StreamEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#"{"path":"temp.md","content":""#.to_string(),
            });
            sink.emit(StreamEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: content,
            });
            sink.emit(StreamEvent::ToolCallArgumentsDelta {
                index: 0,
                delta: r#""}"#.to_string(),
            });
            let response = tool_call_completion(String::new(), vec![call]);
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

impl ProviderClient for WriteArgumentCompletedProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
            }
            sink.emit(StreamEvent::TurnStarted);
            if has_tool_result {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                return Ok(response);
            }

            let call = ToolCall::new("call-write", "write", write_arguments_for_token_tests());
            sink.emit(StreamEvent::ToolCallStarted {
                index: 0,
                call_id: "call-write".to_string(),
                name: "write".to_string(),
            });
            sink.emit(StreamEvent::ToolCallCompleted {
                index: 0,
                call: call.clone(),
            });
            let response = tool_call_completion(String::new(), vec![call]);
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

impl ProviderClient for WriteArgumentResponseOnlyProvider {
    fn stream_prompt<'a>(
        &'a self,
        request: &'a PromptRequest,
        sink: &'a mut (dyn StreamEventSink + Send),
    ) -> ProviderFuture<'a, Result<PromptCompletion, ProviderError>> {
        Box::pin(async move {
            let has_tool_result = has_tool_result(&request.items);
            {
                let mut calls = self.calls.lock().expect("fake lock should not poison");
                *calls += 1;
            }
            sink.emit(StreamEvent::TurnStarted);
            if has_tool_result {
                sink.emit(StreamEvent::TextDelta("done".to_string()));
                let response = text_completion(Role::Assistant, "done");
                sink.emit(StreamEvent::TurnCompleted(response.clone()));
                return Ok(response);
            }

            let call = ToolCall::new("call-write", "write", write_arguments_for_token_tests());
            let response = tool_call_completion(String::new(), vec![call]);
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
