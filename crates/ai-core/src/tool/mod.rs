use serde_json::Value;

/// `ToolDefinition` is the provider-visible schema for a callable tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl ToolDefinition {
    /// `new` creates a provider-visible tool definition.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
        }
    }
}

/// `ToolCall` describes one model-requested function/tool invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    /// `new` creates a tool call using provider-native call identity.
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// `ToolResult` is the provider-context representation of an executed tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    pub details: Option<Value>,
}

impl ToolResult {
    /// `success` creates a successful tool result for provider context.
    pub fn success(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            content: content.into(),
            is_error: false,
            details,
        }
    }

    /// `error` creates a failed tool result for provider context.
    pub fn error(
        call_id: impl Into<String>,
        name: impl Into<String>,
        content: impl Into<String>,
        details: Option<Value>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            content: content.into(),
            is_error: true,
            details,
        }
    }
}
