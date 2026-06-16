use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use provider_protocol::{
    ContentBlock, ConversationItem, ModelDescriptor, PromptCompletion, PromptRequest,
    ProviderCapabilities, ProviderClient, ProviderError, ProviderFuture, Role, StreamEvent,
    StreamEventSink, TokenUsage, ToolCall,
};
use runtime_domain::token_count::StreamingTokenProgress;
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall as RuntimeToolCall, ToolDefinition, ToolExecutionContext,
    ToolExecutionFuture, ToolExecutorRegistry, ToolKind, ToolPermissionDecision,
    ToolPermissionFuture, ToolPermissionHandler, ToolPermissionPolicy, ToolPermissionPreview,
    ToolPermissionRequest, ToolProgress, ToolResult, ToolTerminalSnapshot,
};

use super::{ToolLoopClock, ToolLoopOptions, ToolLoopProgress, run_tool_loop};
use crate::error::ToolLoopError;

fn text_completion(role: Role, text: &str) -> PromptCompletion {
    PromptCompletion::new(
        vec![ConversationItem::text(role, text)],
        provider_protocol::FinishReason::Stop,
        None,
    )
}

fn text_completion_with_usage(role: Role, text: &str, usage: TokenUsage) -> PromptCompletion {
    PromptCompletion::new(
        vec![ConversationItem::text(role, text)],
        provider_protocol::FinishReason::Stop,
        Some(usage),
    )
}

fn tool_call_completion(text: String, calls: Vec<ToolCall>) -> PromptCompletion {
    PromptCompletion::new(
        vec![ConversationItem::assistant_with_tool_calls(text, calls)],
        provider_protocol::FinishReason::ToolCalls,
        None,
    )
}

fn tool_call_completion_with_usage(
    text: String,
    calls: Vec<ToolCall>,
    usage: TokenUsage,
) -> PromptCompletion {
    PromptCompletion::new(
        vec![ConversationItem::assistant_with_tool_calls(text, calls)],
        provider_protocol::FinishReason::ToolCalls,
        Some(usage),
    )
}

fn has_tool_result(items: &[ConversationItem]) -> bool {
    items
        .iter()
        .any(|item| matches!(item, ConversationItem::ToolResult { .. }))
}
