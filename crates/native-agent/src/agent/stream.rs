use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use genai::{
    Client, ModelSpec,
    chat::{ChatOptions, ChatRequest, ChatStreamEvent, StreamEnd},
};

use super::{
    NativeAgentError, NativeAgentRequest,
    response::{NativeAgentCompletion, NativeAgentResponse},
    tool_mapping::{genai_tools_for_registry, runtime_tool_call_from_genai},
};
use crate::{NativeLlmPerformanceMetrics, NativeLlmProgress, llm::ChatMessageGenAiExt};
use mo_core::{token_count::StreamingTokenProgress, tools::RuntimeToolCall};

pub(crate) fn chat_request_for_agent(request: &NativeAgentRequest) -> ChatRequest {
    let chat_request = request.llm_request();
    let mut genai_request = ChatRequest::new(
        chat_request
            .messages
            .clone()
            .into_iter()
            .map(|message| message.into_genai())
            .collect(),
    );
    if !request.tools().is_empty() {
        genai_request = genai_request.with_tools(genai_tools_for_registry(request.tools()));
    }
    genai_request
}

pub(crate) fn chat_options_for_agent(request: &NativeAgentRequest) -> ChatOptions {
    let mut options = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_reasoning_content(true)
        .with_capture_usage(true);
    if !request.tools().is_empty() {
        options = options.with_capture_tool_calls(true);
    }
    options
}

pub(crate) async fn execute_agent_chat_request<F>(
    client: &Client,
    model: ModelSpec,
    chat_request: ChatRequest,
    model_id: String,
    options: &ChatOptions,
    cancellation: &tokio_util::sync::CancellationToken,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeLlmProgress),
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }

    let stream_response = tokio::select! {
        _ = cancellation.cancelled() => return Err(NativeAgentError::Cancelled),
        response = client.exec_chat_stream(model, chat_request, Some(options)) => response?,
    };

    let mut stream = stream_response.stream;
    let mut output = NativeAgentAccumulator::new(model_id);

    loop {
        let event = tokio::select! {
            _ = cancellation.cancelled() => return Err(NativeAgentError::Cancelled),
            event = stream.next() => event,
        };
        let Some(event) = event else {
            break;
        };

        match event? {
            ChatStreamEvent::Start => {
                output.mark_request_started(Instant::now());
            }
            ChatStreamEvent::Chunk(chunk) => {
                output.observe_content_chunk(&chunk.content, Instant::now(), on_progress);
            }
            ChatStreamEvent::ReasoningChunk(chunk) => {
                output.observe_reasoning_chunk(&chunk.content, Instant::now(), on_progress);
            }
            ChatStreamEvent::ThoughtSignatureChunk(_) | ChatStreamEvent::ToolCallChunk(_) => {}
            ChatStreamEvent::End(end) => {
                output.capture_stream_end(&end);
                output.stream_end = Some(end);
                break;
            }
        }
    }

    if let Some(total_tokens) = output.progress.flush(Instant::now()) {
        on_progress(NativeLlmProgress::OutputTokens { total_tokens });
    }

    Ok(output.finish_at(Instant::now(), None))
}

struct NativeAgentAccumulator {
    content: String,
    reasoning_content: String,
    tool_calls: Vec<RuntimeToolCall>,
    progress: StreamingTokenProgress,
    is_thinking: bool,
    reasoning_started_at: Option<Instant>,
    reasoning_finished_at: Option<Instant>,
    request_started_at: Option<Instant>,
    first_token_at: Option<Instant>,
    stream_end: Option<StreamEnd>,
}

impl NativeAgentAccumulator {
    fn new(model_id: String) -> Self {
        Self {
            content: String::new(),
            reasoning_content: String::new(),
            tool_calls: Vec::new(),
            progress: StreamingTokenProgress::new(model_id),
            is_thinking: false,
            reasoning_started_at: None,
            reasoning_finished_at: None,
            request_started_at: None,
            first_token_at: None,
            stream_end: None,
        }
    }

    fn mark_request_started(&mut self, now: Instant) {
        self.request_started_at = Some(now);
    }

    fn observe_content_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeLlmProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if self.is_thinking {
            self.is_thinking = false;
            self.reasoning_finished_at = Some(now);
            on_progress(NativeLlmProgress::Thinking { is_thinking: false });
        }
        self.content.push_str(content);
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_reasoning_chunk(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeLlmProgress),
    ) {
        if content.is_empty() {
            return;
        }
        self.first_token_at.get_or_insert(now);
        if !self.is_thinking {
            self.is_thinking = true;
            self.reasoning_started_at.get_or_insert(now);
            on_progress(NativeLlmProgress::Thinking { is_thinking: true });
        }
        self.reasoning_finished_at = Some(now);
        self.reasoning_content.push_str(content);
        self.observe_token_delta(content, now, on_progress);
    }

    fn observe_token_delta(
        &mut self,
        content: &str,
        now: Instant,
        on_progress: &mut impl FnMut(NativeLlmProgress),
    ) {
        if let Some(total_tokens) = self.progress.observe_delta(content, now) {
            on_progress(NativeLlmProgress::OutputTokens { total_tokens });
        }
    }

    fn capture_stream_end(&mut self, end: &StreamEnd) {
        if self.content.is_empty()
            && let Some(captured) = end.captured_content.as_ref()
            && let Some(captured_text) = captured.joined_texts()
        {
            self.content = captured_text;
        }
        if self.reasoning_content.is_empty()
            && let Some(captured_reasoning) = end.captured_reasoning_content.as_ref()
        {
            self.reasoning_content = captured_reasoning.clone();
        }
        if let Some(captured_tool_calls) = end.captured_tool_calls() {
            self.tool_calls = captured_tool_calls
                .into_iter()
                .cloned()
                .map(runtime_tool_call_from_genai)
                .collect();
        }
    }

    fn finish_at(
        self,
        finished_at: Instant,
        output_tokens: Option<usize>,
    ) -> NativeAgentCompletion {
        let metrics = self.performance_metrics(finished_at, output_tokens);
        let reasoning_content = trim_outer_blank_lines(&self.reasoning_content);
        let reasoning_duration = self.reasoning_duration();
        let content = if reasoning_content.is_empty() {
            self.content.clone()
        } else {
            trim_outer_blank_lines(&self.content)
        };
        NativeAgentCompletion {
            response: NativeAgentResponse {
                content,
                reasoning_content: (!reasoning_content.is_empty()).then_some(reasoning_content),
                reasoning_duration,
                tool_calls: self.tool_calls,
                tool_results: Vec::new(),
            },
            metrics,
            stream_end: self.stream_end,
        }
    }

    fn performance_metrics(
        &self,
        finished_at: Instant,
        output_tokens: Option<usize>,
    ) -> Option<NativeLlmPerformanceMetrics> {
        let request_started_at = self.request_started_at?;
        let first_token_at = self.first_token_at?;
        Some(NativeLlmPerformanceMetrics {
            latency: first_token_at.saturating_duration_since(request_started_at),
            output_tokens: output_tokens.unwrap_or_else(|| self.progress.total_tokens()),
            duration: finished_at.saturating_duration_since(request_started_at),
        })
    }

    fn reasoning_duration(&self) -> Option<Duration> {
        if self.reasoning_content.trim().is_empty() {
            return None;
        }

        let started_at = self.reasoning_started_at?;
        let finished_at = self.reasoning_finished_at.unwrap_or(started_at);
        Some(finished_at.saturating_duration_since(started_at))
    }
}

fn trim_outer_blank_lines(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .expect("start exists when at least one non-blank line exists");

    lines[start..=end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChatMessage, ProviderKind};
    use genai::chat::{MessageContent, ToolCall};
    use mo_core::tools::{RuntimeToolDefinition, RuntimeToolRegistry};

    #[test]
    fn agent_chat_request_includes_registered_tools() {
        let mut tools = RuntimeToolRegistry::new();
        tools.insert(RuntimeToolDefinition::new("read_file").with_input_schema(
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        ));
        let request = NativeAgentRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("read Cargo.toml".to_string())],
        )
        .with_tools(tools);

        let chat_request = chat_request_for_agent(&request);
        let tool = chat_request
            .tools
            .as_ref()
            .and_then(|tools| tools.first())
            .expect("agent request should include tools");

        assert_eq!(chat_request.messages.len(), 1);
        assert_eq!(tool.name.as_str(), "read_file");
        assert_eq!(tool.strict, Some(true));
    }

    #[test]
    fn agent_chat_options_capture_tool_calls_when_tools_are_registered() {
        let mut tools = RuntimeToolRegistry::new();
        tools.insert(RuntimeToolDefinition::new("read_file"));
        let request = NativeAgentRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("read Cargo.toml".to_string())],
        )
        .with_tools(tools);

        let options = chat_options_for_agent(&request);

        assert_eq!(options.capture_content, Some(true));
        assert_eq!(options.capture_reasoning_content, Some(true));
        assert_eq!(options.capture_usage, Some(true));
        assert_eq!(options.capture_tool_calls, Some(true));
    }

    #[test]
    fn accumulator_keeps_tool_calls_from_stream_end() {
        let mut accumulator = NativeAgentAccumulator::new("qwen3".to_string());
        let end = StreamEnd {
            captured_content: Some(MessageContent::from_tool_calls(vec![ToolCall {
                call_id: "call-1".to_string(),
                fn_name: "read_file".to_string(),
                fn_arguments: serde_json::json!({ "path": "Cargo.toml" }),
                thought_signatures: None,
            }])),
            ..Default::default()
        };

        accumulator.capture_stream_end(&end);
        let completion = accumulator.finish_at(Instant::now(), None);

        assert_eq!(completion.response.tool_calls.len(), 1);
        assert_eq!(completion.response.tool_calls[0].call_id, "call-1");
        assert_eq!(completion.response.tool_calls[0].name, "read_file");
        assert_eq!(
            completion.response.tool_calls[0].arguments,
            serde_json::json!({ "path": "Cargo.toml" })
        );
    }
}
