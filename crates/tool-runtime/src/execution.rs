use serde_json::Value;

/// `ToolCall` 描述模型发起的一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

impl ToolCall {
    /// `new` 创建一次工具调用描述。
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// `ToolResult` 描述工具执行后回传给 runtime 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub display_content: Option<String>,
    pub is_error: bool,
    pub details: Option<Value>,
    pub terminate: bool,
}

impl ToolResult {
    /// `success` 创建成功工具结果。
    pub fn success(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            display_content: None,
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
            display_content: None,
            is_error: true,
            details: None,
            terminate: false,
        }
    }

    /// `with_display_content` 设置仅供 runtime/TUI 展示的内容。
    pub fn with_display_content(mut self, display_content: impl Into<String>) -> Self {
        self.display_content = Some(display_content.into());
        self
    }
}
