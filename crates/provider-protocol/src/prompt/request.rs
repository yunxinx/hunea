use serde_json::Value;

use crate::{message::ConversationItem, tool::ToolDefinition};

/// `PromptOptions` contains provider-call options that are independent of session state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromptOptions {
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub top_p: Option<f32>,
    pub metadata: Option<Value>,
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
