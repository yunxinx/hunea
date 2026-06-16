use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// `ToolDefinition` is the provider-visible schema for a callable tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// tool call arguments 解析错误。
#[derive(Debug, Error)]
#[error("failed to parse arguments for tool `{tool_name}`: {source}")]
pub struct ToolCallArgumentsError {
    tool_name: String,
    source: serde_json::Error,
}

impl ToolCallArgumentsError {
    /// 返回发生解析错误的 tool 名称。
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

impl ToolCall {
    /// `new` creates a tool call using provider-assigned call identity.
    pub fn new(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    /// 解析 arguments JSON 为指定类型。
    pub fn parsed_arguments<T>(&self) -> Result<T, ToolCallArgumentsError>
    where
        T: DeserializeOwned,
    {
        serde_json::from_str(self.arguments_json()).map_err(|error| ToolCallArgumentsError {
            tool_name: self.name.clone(),
            source: error,
        })
    }

    /// 解析 arguments JSON 为 `serde_json::Value`。
    pub fn parsed_arguments_value(&self) -> Result<Value, ToolCallArgumentsError> {
        self.parsed_arguments()
    }

    fn arguments_json(&self) -> &str {
        if self.arguments.trim().is_empty() {
            "{}"
        } else {
            self.arguments.as_str()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCall;

    #[test]
    fn parsed_arguments_value_returns_json_with_tool_context_on_error() {
        use std::error::Error as _;

        let call = ToolCall::new("call-1", "bash", "not valid json");

        let error = call
            .parsed_arguments_value()
            .expect_err("invalid JSON should be rejected");

        assert_eq!(error.tool_name(), "bash");
        assert!(error.to_string().contains("failed to parse arguments"));
        assert!(error.to_string().contains("bash"));
        assert!(
            error.source().is_some(),
            "arguments parse errors should preserve serde_json source"
        );
    }

    #[test]
    fn parsed_arguments_deserializes_typed_arguments() {
        #[derive(Debug, serde::Deserialize, PartialEq)]
        struct ReadArguments {
            path: String,
        }

        let call = ToolCall::new("call-1", "read", r#"{"path":"Cargo.toml"}"#);

        let arguments: ReadArguments = call
            .parsed_arguments()
            .expect("valid JSON should deserialize");

        assert_eq!(
            arguments,
            ReadArguments {
                path: "Cargo.toml".to_string(),
            }
        );
    }

    #[test]
    fn tool_call_new_accepts_borrowed_arguments() {
        let call = ToolCall::new("call-1", "read", "{}");

        assert_eq!(call.arguments, "{}");
    }
}

/// `ToolResult` is the provider-context representation of an executed tool result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
