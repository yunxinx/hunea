use crate::{message::Message, tool::ToolCall};

use super::{FinishReason, TokenUsage};

/// `PromptResponse` is the aggregate result of one provider call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptResponse {
    pub message: Message,
    pub finish_reason: FinishReason,
    pub usage: Option<TokenUsage>,
    pub tool_calls: Vec<ToolCall>,
}

impl PromptResponse {
    /// `new` creates an aggregate provider response.
    pub fn new(
        message: Message,
        finish_reason: FinishReason,
        usage: Option<TokenUsage>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            message,
            finish_reason,
            usage,
            tool_calls,
        }
    }
}
