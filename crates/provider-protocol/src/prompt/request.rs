use serde_json::Value;

use crate::{message::ConversationItem, tool::ToolDefinition};

/// OpenAI-compatible provider 支持的 prompt cache 保留时长。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheRetention {
    /// 请求 provider 将 prompt cache 保留 24 小时。
    Long24h,
}

impl PromptCacheRetention {
    /// 返回 OpenAI-compatible provider 使用的字段值。
    pub const fn as_openai_value(self) -> &'static str {
        match self {
            Self::Long24h => "24h",
        }
    }
}

/// `PromptOptions` contains provider-call options shared by provider adapters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptOptions {
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub metadata: Option<Value>,
    /// 稳定的 prompt cache 亲和键，用于让 provider 将同一会话路由到同一前缀缓存。
    pub prompt_cache_key: Option<String>,
    /// OpenAI-compatible provider 的 prompt cache 保留时长提示。
    pub prompt_cache_retention: Option<PromptCacheRetention>,
}

/// `PromptRequest` is one provider call input.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptRequest {
    pub model: String,
    pub items: Vec<ConversationItem>,
    pub tools: Vec<ToolDefinition>,
    pub options: PromptOptions,
}

impl PromptRequest {
    /// `new` creates a provider prompt request.
    pub fn new(model: impl Into<String>, items: Vec<ConversationItem>) -> Self {
        Self {
            model: model.into(),
            items,
            tools: Vec::new(),
            options: PromptOptions::default(),
        }
    }

    /// `with_tools` attaches provider-visible tool definitions.
    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }
}
