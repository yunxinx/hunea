use genai::chat::{ChatMessage as GenAiChatMessage, ChatRequest, StreamEnd};

use super::{
    NativeAgentError, NativeAgentRequest,
    response::{NativeAgentCompletion, NativeAgentProgress},
    stream::{chat_options_for_agent, chat_request_for_agent, execute_agent_chat_request},
    tool_mapping::genai_tool_response_from_runtime,
};
use crate::runtime::{
    native::{NativeLlmProgress, client_for_request, model_spec_for_request},
    tools::{RuntimeToolCall, RuntimeToolExecutor, RuntimeToolResult},
};

const MAX_AGENT_TOOL_ROUNDS: usize = 8;

pub(crate) async fn send_agent_loop_with_cancellation_and_token_progress<F>(
    request: &NativeAgentRequest,
    executor: &dyn RuntimeToolExecutor,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeLlmProgress),
{
    send_agent_loop_with_cancellation_and_progress(request, executor, cancellation, |progress| {
        match progress {
            NativeAgentProgress::OutputTokens { total_tokens } => {
                on_progress(NativeLlmProgress::OutputTokens { total_tokens });
            }
            NativeAgentProgress::Thinking { is_thinking } => {
                on_progress(NativeLlmProgress::Thinking { is_thinking });
            }
            NativeAgentProgress::ToolExecutionStarted { .. }
            | NativeAgentProgress::ToolExecutionFinished { .. } => {}
        }
    })
    .await
}

pub(crate) async fn send_agent_loop_with_cancellation_and_progress<F>(
    request: &NativeAgentRequest,
    executor: &dyn RuntimeToolExecutor,
    cancellation: &tokio_util::sync::CancellationToken,
    mut on_progress: F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress),
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }

    let client = client_for_request(request.llm_request());
    let model = model_spec_for_request(request.llm_request())?;
    let options = chat_options_for_agent(request);
    let mut chat_request = chat_request_for_agent(request);
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    for tool_round in 0..=MAX_AGENT_TOOL_ROUNDS {
        let mut on_chat_progress = |progress| match progress {
            NativeLlmProgress::OutputTokens { total_tokens } => {
                on_progress(NativeAgentProgress::OutputTokens { total_tokens });
            }
            NativeLlmProgress::Thinking { is_thinking } => {
                on_progress(NativeAgentProgress::Thinking { is_thinking });
            }
        };
        let mut completion = execute_agent_chat_request(
            &client,
            model.clone(),
            chat_request.clone(),
            request.llm_request().model_id.clone(),
            &options,
            cancellation,
            &mut on_chat_progress,
        )
        .await?;

        if completion.response.tool_calls.is_empty() {
            completion.response.tool_calls = tool_calls;
            completion.response.tool_results = tool_results;
            return Ok(completion);
        }

        if tool_round == MAX_AGENT_TOOL_ROUNDS {
            return Err(NativeAgentError::ToolLoopLimitExceeded {
                max_tool_rounds: MAX_AGENT_TOOL_ROUNDS,
            });
        }

        let stream_end = completion
            .stream_end
            .as_ref()
            .ok_or(NativeAgentError::MissingToolCallCapture)?;
        let current_tool_calls = completion.response.tool_calls.clone();
        let current_tool_results = execute_tool_batch(
            executor,
            &current_tool_calls,
            cancellation,
            &mut on_progress,
        )
        .await?;
        let should_stop = should_terminate_tool_batch(&current_tool_results);

        chat_request =
            append_tool_results_to_chat_request(chat_request, stream_end, &current_tool_results);
        tool_calls.extend(current_tool_calls);
        tool_results.extend(current_tool_results);

        if should_stop {
            completion.response.tool_calls = tool_calls;
            completion.response.tool_results = tool_results;
            return Ok(completion);
        }
    }

    Err(NativeAgentError::ToolLoopLimitExceeded {
        max_tool_rounds: MAX_AGENT_TOOL_ROUNDS,
    })
}

fn append_tool_results_to_chat_request(
    mut chat_request: ChatRequest,
    end: &StreamEnd,
    results: &[RuntimeToolResult],
) -> ChatRequest {
    if let Some(content) = end.captured_content.as_ref() {
        chat_request
            .messages
            .push(GenAiChatMessage::assistant(content.clone()));
    } else if let Some(calls_ref) = end.captured_tool_calls() {
        let calls = calls_ref.into_iter().cloned().collect::<Vec<_>>();
        if !calls.is_empty() {
            chat_request.messages.push(GenAiChatMessage::from(calls));
        }
    }

    let responses = results
        .iter()
        .map(genai_tool_response_from_runtime)
        .collect::<Vec<_>>();
    chat_request
        .messages
        .push(GenAiChatMessage::from(responses));
    chat_request
}

async fn execute_tool_batch(
    executor: &dyn RuntimeToolExecutor,
    calls: &[RuntimeToolCall],
    cancellation: &tokio_util::sync::CancellationToken,
    on_progress: &mut impl FnMut(NativeAgentProgress),
) -> Result<Vec<RuntimeToolResult>, NativeAgentError> {
    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        if cancellation.is_cancelled() {
            return Err(NativeAgentError::Cancelled);
        }
        on_progress(NativeAgentProgress::ToolExecutionStarted { call: call.clone() });
        let result = executor.execute_tool(call.clone(), cancellation).await;
        if cancellation.is_cancelled() {
            return Err(NativeAgentError::Cancelled);
        }
        on_progress(NativeAgentProgress::ToolExecutionFinished {
            call: call.clone(),
            result: result.clone(),
        });
        results.push(result);
    }
    Ok(results)
}

fn should_terminate_tool_batch(results: &[RuntimeToolResult]) -> bool {
    !results.is_empty() && results.iter().all(|result| result.terminate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{
        native::{ChatMessage, ProviderKind},
        tools::{RuntimeToolExecutionFuture, RuntimeToolExecutor, RuntimeToolResult},
    };
    use genai::chat::{MessageContent, ToolCall};
    use std::sync::{Arc, Mutex};

    #[test]
    fn appends_tool_results_as_single_tool_message_after_assistant_tool_calls() {
        let request = NativeAgentRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            vec![ChatMessage::user("read two files".to_string())],
        );
        let chat_request = chat_request_for_agent(&request);
        let end = StreamEnd {
            captured_content: Some(MessageContent::from_tool_calls(vec![
                ToolCall {
                    call_id: "call-1".to_string(),
                    fn_name: "read_file".to_string(),
                    fn_arguments: serde_json::json!({ "path": "Cargo.toml" }),
                    thought_signatures: None,
                },
                ToolCall {
                    call_id: "call-2".to_string(),
                    fn_name: "read_file".to_string(),
                    fn_arguments: serde_json::json!({ "path": "README.md" }),
                    thought_signatures: None,
                },
            ])),
            ..Default::default()
        };
        let results = vec![
            RuntimeToolResult::success("call-1", "cargo"),
            RuntimeToolResult::success("call-2", "readme"),
        ];

        let chat_request = append_tool_results_to_chat_request(chat_request, &end, &results);

        assert_eq!(chat_request.messages.len(), 3);
        assert_eq!(
            chat_request.messages[1].role,
            genai::chat::ChatRole::Assistant
        );
        assert_eq!(chat_request.messages[1].content.tool_calls().len(), 2);
        assert_eq!(chat_request.messages[2].role, genai::chat::ChatRole::Tool);
        let tool_responses = chat_request.messages[2].content.tool_responses();
        assert_eq!(tool_responses.len(), 2);
        assert_eq!(tool_responses[0].call_id, "call-1");
        assert_eq!(tool_responses[0].content, "cargo");
        assert_eq!(tool_responses[1].call_id, "call-2");
        assert_eq!(tool_responses[1].content, "readme");
    }

    struct RecordingToolExecutor {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl RuntimeToolExecutor for RecordingToolExecutor {
        fn execute_tool<'a>(
            &'a self,
            call: RuntimeToolCall,
            _cancellation: &'a tokio_util::sync::CancellationToken,
        ) -> RuntimeToolExecutionFuture<'a> {
            let seen = self.seen.clone();
            Box::pin(async move {
                seen.lock()
                    .expect("recording executor lock should not be poisoned")
                    .push(call.call_id.clone());
                RuntimeToolResult::success(call.call_id, format!("{} done", call.name))
            })
        }
    }

    #[tokio::test]
    async fn executes_tool_batch_in_model_call_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let executor = RecordingToolExecutor { seen: seen.clone() };
        let calls = vec![
            RuntimeToolCall::new("call-1", "first", serde_json::json!({})),
            RuntimeToolCall::new("call-2", "second", serde_json::json!({})),
        ];

        let results = execute_tool_batch(
            &executor,
            &calls,
            &tokio_util::sync::CancellationToken::new(),
            &mut |_| {},
        )
        .await
        .expect("tool batch should execute");

        assert_eq!(
            *seen
                .lock()
                .expect("recording executor lock should not be poisoned"),
            vec!["call-1".to_string(), "call-2".to_string()]
        );
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            RuntimeToolResult::success("call-1", "first done")
        );
        assert_eq!(
            results[1],
            RuntimeToolResult::success("call-2", "second done")
        );
    }

    #[tokio::test]
    async fn execute_tool_batch_emits_start_and_finish_progress() {
        let executor = RecordingToolExecutor {
            seen: Arc::new(Mutex::new(Vec::new())),
        };
        let calls = vec![RuntimeToolCall::new(
            "call-1",
            "first",
            serde_json::json!({}),
        )];
        let mut events = Vec::new();

        let _ = execute_tool_batch(
            &executor,
            &calls,
            &tokio_util::sync::CancellationToken::new(),
            &mut |event| events.push(event),
        )
        .await
        .expect("tool batch should execute");

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            NativeAgentProgress::ToolExecutionStarted { call } if call.call_id == "call-1"
        ));
        assert!(matches!(
            &events[1],
            NativeAgentProgress::ToolExecutionFinished { call, result }
                if call.call_id == "call-1" && !result.is_error
        ));
    }
}
