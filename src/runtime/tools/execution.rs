use serde_json::Value;

/// `RuntimeToolCall` 描述模型发起的一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl RuntimeToolCall {
    /// `new` 创建一次工具调用描述。
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// `RuntimeToolResult` 描述工具执行后回传给 runtime 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    pub details: Option<Value>,
    pub terminate: bool,
}

impl RuntimeToolResult {
    /// `success` 创建成功工具结果。
    pub fn success(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: false,
            details: None,
            terminate: false,
        }
    }

    /// `error` 创建失败工具结果。
    pub fn error(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: true,
            details: None,
            terminate: false,
        }
    }
}
