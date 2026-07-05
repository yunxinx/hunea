use serde::Deserialize;
use serde_json::Value;
use std::fmt;

/// `ToolCall` 描述模型发起的一次工具调用。
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
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
#[must_use]
pub struct ToolResult {
    call_id: String,
    content: ToolResultContentBlocks,
    display_content: Option<String>,
    outcome: ToolResultOutcome,
    details: Option<Value>,
}

/// `ToolResultOutcome` 表达工具结果对 runtime 的控制含义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolResultOutcome {
    Success,
    Error,
    Terminate,
}

/// `ToolResultContentBlocks` 保存工具结果的结构化内容，并提供文本摘要接口。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResultContentBlocks {
    blocks: Vec<ToolResultContent>,
}

/// `ToolResultContent` 是工具结果的模型可见结构化内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolResultContent {
    Text(String),
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
        detail: Option<ToolImageDetail>,
    },
}

/// `ToolImageDetail` 描述工具返回图片希望 provider 使用的细节等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolImageDetail {
    High,
    Original,
}

impl ToolResultContentBlocks {
    /// `new` 创建结构化工具结果内容列表。
    pub fn new(blocks: Vec<ToolResultContent>) -> Self {
        Self { blocks }
    }

    /// `as_slice` 返回原始结构化块。
    pub fn as_slice(&self) -> &[ToolResultContent] {
        &self.blocks
    }

    /// `iter` 遍历原始结构化块。
    pub fn iter(&self) -> impl Iterator<Item = &ToolResultContent> {
        self.blocks.iter()
    }

    /// `is_empty` 返回内容是否为空。
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// `text` 返回所有文本块拼接后的摘要。
    pub fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|content| match content {
                ToolResultContent::Text(text) => Some(text.as_str()),
                ToolResultContent::Image { .. } => None,
            })
            .collect::<String>()
    }

    /// `contains` 在文本摘要中搜索子串。
    pub fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }

    /// `ends_with` 判断文本摘要后缀。
    pub fn ends_with(&self, needle: &str) -> bool {
        self.text().ends_with(needle)
    }
}

impl From<Vec<ToolResultContent>> for ToolResultContentBlocks {
    fn from(blocks: Vec<ToolResultContent>) -> Self {
        Self::new(blocks)
    }
}

impl fmt::Display for ToolResultContentBlocks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text())
    }
}

impl ToolResult {
    /// `success` 创建成功工具结果。
    pub fn success(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::success_content(call_id, vec![ToolResultContent::Text(content.into())])
    }

    /// `success_content` 创建带结构化内容的成功工具结果。
    pub fn success_content(call_id: impl Into<String>, content: Vec<ToolResultContent>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            display_content: None,
            outcome: ToolResultOutcome::Success,
            details: None,
        }
    }

    /// `error` 创建失败工具结果。
    pub fn error(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: vec![ToolResultContent::Text(content.into())].into(),
            display_content: None,
            outcome: ToolResultOutcome::Error,
            details: None,
        }
    }

    /// `terminate` 创建终止当前工具循环的工具结果。
    pub fn terminate(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: vec![ToolResultContent::Text(content.into())].into(),
            display_content: None,
            outcome: ToolResultOutcome::Terminate,
            details: None,
        }
    }

    /// `with_display_content` 设置仅供 runtime/TUI 展示的内容。
    pub fn with_display_content(mut self, display_content: impl Into<String>) -> Self {
        self.display_content = Some(display_content.into());
        self
    }

    /// `with_details` 附加结构化执行细节，供 runtime/TUI 使用。
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// `outcome` 返回工具结果对 runtime 的控制含义。
    pub fn outcome(&self) -> ToolResultOutcome {
        self.outcome
    }

    /// `call_id` 返回该结果对应的工具调用 ID。
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// `content` 返回模型可见的结构化工具结果内容。
    pub fn content(&self) -> &ToolResultContentBlocks {
        &self.content
    }

    /// `is_error` 返回工具是否失败。
    pub fn is_error(&self) -> bool {
        self.outcome == ToolResultOutcome::Error
    }

    /// `terminates` 返回工具结果是否终止当前工具循环。
    pub fn terminates(&self) -> bool {
        self.outcome == ToolResultOutcome::Terminate
    }

    /// `details` 返回结构化执行细节。
    pub fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }

    /// `display_content` 返回仅供 runtime/TUI 展示的内容。
    pub fn display_content(&self) -> Option<&str> {
        self.display_content.as_deref()
    }

    /// `text_content` 返回工具结果中的文本内容。
    pub fn text_content(&self) -> String {
        self.content.text()
    }

    /// `display_text` 返回展示文本，优先使用 display-only 内容。
    pub fn display_text(&self) -> String {
        self.display_content
            .clone()
            .unwrap_or_else(|| self.text_content())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_outcome_has_single_control_state() {
        let success = ToolResult::success("success-call", "ok");
        assert_eq!(success.outcome(), ToolResultOutcome::Success);
        assert!(!success.is_error());
        assert!(!success.terminates());

        let error = ToolResult::error("error-call", "failed");
        assert_eq!(error.outcome(), ToolResultOutcome::Error);
        assert!(error.is_error());
        assert!(!error.terminates());

        let terminate = ToolResult::terminate("terminate-call", "stop");
        assert_eq!(terminate.outcome(), ToolResultOutcome::Terminate);
        assert!(!terminate.is_error());
        assert!(terminate.terminates());
    }

    #[test]
    fn tool_result_exposes_read_only_identity_and_content() {
        let result =
            ToolResult::success("call-1", "visible output").with_display_content("display output");

        assert_eq!(result.call_id(), "call-1");
        assert!(result.content().contains("visible output"));
        assert_eq!(result.text_content(), "visible output");
        assert_eq!(result.display_text(), "display output");
    }
}
