use crate::message::ConversationItem;

use super::{FinishReason, TokenUsage};

/// item-centric 协议的单 turn 完整产出。
///
/// `tool_calls` 字段已删除——从 `items` 中的 assistant item 提取。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompletion {
    pub items: Vec<ConversationItem>,
    pub finish_reason: FinishReason,
    pub usage: Option<TokenUsage>,
}

impl PromptCompletion {
    /// 创建一个 provider 单 turn 完成结果。
    pub fn new(
        items: Vec<ConversationItem>,
        finish_reason: FinishReason,
        usage: Option<TokenUsage>,
    ) -> Self {
        Self {
            items,
            finish_reason,
            usage,
        }
    }
}
