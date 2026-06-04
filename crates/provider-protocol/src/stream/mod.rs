use crate::{prompt::PromptCompletion, tool::ToolCall};

/// `StreamEvent` is the provider-neutral event stream exposed to runtime code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    TurnStarted,
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallStarted {
        index: usize,
        call_id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        index: usize,
        delta: String,
    },
    ToolCallCompleted {
        index: usize,
        call: ToolCall,
    },
    UsageUpdated(crate::prompt::TokenUsage),
    TurnCompleted(PromptCompletion),
}
