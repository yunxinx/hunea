/// `FinishReason` normalizes provider completion stop reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Error,
    Other(String),
}

impl FinishReason {
    /// `is_tool_call` returns true when the runtime should execute tool calls.
    pub const fn is_tool_call(&self) -> bool {
        matches!(self, Self::ToolCalls)
    }
}
