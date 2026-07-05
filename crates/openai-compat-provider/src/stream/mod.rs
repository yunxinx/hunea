mod chat;
mod responses;
mod sse;

use provider_protocol::{FinishReason, ProviderError, ToolCall};
use serde::Deserialize;

pub(crate) use chat::OpenAiStreamState;
pub(crate) use responses::OpenAiResponsesStreamState;
pub(crate) use sse::OpenAiSseDecoder;

#[derive(Debug, Default)]
struct PartialToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PartialToolCall {
    fn to_tool_call(&self, index: usize) -> Result<ToolCall, ProviderError> {
        let name = self.name.clone().ok_or_else(|| {
            ProviderError::Protocol(format!("tool call {index} completed without a name"))
        })?;
        let call_id = self.call_id.clone().ok_or_else(|| {
            ProviderError::Protocol(format!("tool call {index} completed without an id"))
        })?;
        let arguments = if self.arguments.trim().is_empty() {
            "{}".to_string()
        } else {
            self.arguments.clone()
        };
        Ok(ToolCall::new(call_id, name, arguments))
    }
}

fn finish_reason_from_openai(value: &str) -> FinishReason {
    match value {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        other => FinishReason::Other(other.to_string()),
    }
}

fn finish_reason_from_responses_status(status: Option<&str>) -> FinishReason {
    match status {
        Some("completed") => FinishReason::Stop,
        Some("incomplete") => FinishReason::Length,
        Some(other @ ("failed" | "cancelled")) => FinishReason::Other(other.to_string()),
        Some(other) => FinishReason::Other(other.to_string()),
        None => FinishReason::Stop,
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoiceDelta>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceDelta {
    #[serde(default)]
    delta: OpenAiDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponsesStreamEvent {
    #[serde(rename = "response.output_item.added")]
    OutputItemAdded {
        output_index: usize,
        item: ResponsesOutputItem,
    },
    #[serde(rename = "response.output_item.done")]
    OutputItemDone {
        output_index: usize,
        item: ResponsesOutputItem,
    },
    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.refusal.delta")]
    RefusalDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_text.delta")]
    ReasoningTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_text.delta")]
    ReasoningSummaryTextDelta {
        #[serde(default)]
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.reasoning_summary_part.done")]
    ReasoningSummaryPartDone {
        #[serde(default)]
        output_index: usize,
    },
    #[serde(rename = "response.function_call_arguments.delta")]
    FunctionCallArgumentsDelta {
        output_index: usize,
        #[serde(default)]
        delta: String,
    },
    #[serde(rename = "response.function_call_arguments.done")]
    FunctionCallArgumentsDone {
        output_index: usize,
        #[serde(default)]
        arguments: String,
    },
    #[serde(rename = "response.completed")]
    Completed { response: ResponsesTerminalResponse },
    #[serde(rename = "response.incomplete")]
    Incomplete { response: ResponsesTerminalResponse },
    #[serde(rename = "response.failed")]
    Failed { response: ResponsesTerminalResponse },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        code: String,
        #[serde(default)]
        message: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesOutputItem {
    #[serde(rename = "type")]
    kind: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
    content: Option<Vec<ResponsesOutputContent>>,
    summary: Option<Vec<ResponsesOutputContent>>,
}

impl ResponsesOutputItem {
    fn visible_output_text(&self) -> Option<String> {
        let content = self.content.as_ref()?;
        let text = content
            .iter()
            .filter(|item| matches!(item.kind.as_deref(), Some("output_text" | "refusal")))
            .filter_map(|item| item.text.as_deref())
            .collect::<String>();
        (!text.is_empty()).then_some(text)
    }

    fn visible_reasoning_text(&self) -> Option<String> {
        let blocks = self.summary.as_ref().or(self.content.as_ref())?;
        let text = blocks
            .iter()
            .filter_map(|item| item.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n\n");
        (!text.is_empty()).then_some(text)
    }
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesOutputContent {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResponsesTerminalResponse {
    status: Option<String>,
    usage: Option<ResponsesUsage>,
    error: Option<ResponsesError>,
    incomplete_details: Option<ResponsesIncompleteDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ResponsesError {
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesIncompleteDetails {
    reason: Option<String>,
}

#[cfg(test)]
mod tests;
