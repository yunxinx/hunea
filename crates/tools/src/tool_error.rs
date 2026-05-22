use std::sync::Arc;

/// `ProcessedToolError` 是工具失败跨 provider 回灌前的统一文本契约。
///
/// `assistant_message` 会进入模型上下文，`display_reason` 保留给 UI 层使用。
/// Lumos runtime 在工具执行处保留两个文本，避免 provider 适配层承担业务语义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessedToolError {
    pub assistant_message: String,
    pub display_reason: String,
}

impl ProcessedToolError {
    /// `new` 创建一条已分层的工具错误。
    pub fn new(assistant_message: impl Into<String>, display_reason: impl Into<String>) -> Self {
        Self {
            assistant_message: assistant_message.into(),
            display_reason: display_reason.into(),
        }
    }
}

/// `ToolErrorFormatter` 将工具原始错误转换成模型上下文和 UI 展示可消费的文本。
pub trait ToolErrorFormatter: Send + Sync {
    /// `format_tool_error` 返回清洗后的工具失败信息。
    fn format_tool_error(&self, tool_name: &str, raw_error: &str) -> ProcessedToolError;
}

/// `SharedToolErrorFormatter` 是 runtime tool loop 可克隆持有的 formatter。
pub type SharedToolErrorFormatter = Arc<dyn ToolErrorFormatter>;

/// `DefaultToolErrorFormatter` 提供通用保底清洗，业务层可注入更具体的实现。
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultToolErrorFormatter;

impl ToolErrorFormatter for DefaultToolErrorFormatter {
    fn format_tool_error(&self, tool_name: &str, raw_error: &str) -> ProcessedToolError {
        let display_reason = default_display_reason(tool_name, raw_error);
        ProcessedToolError::new(
            format!("{display_reason}. Hint: Check the tool input and try again."),
            display_reason,
        )
    }
}

fn default_display_reason(tool_name: &str, raw_error: &str) -> String {
    let cleaned = strip_os_error_suffix(raw_error.trim());
    if cleaned.is_empty() {
        format!("Tool failed: {tool_name}")
    } else {
        format!("Tool failed: {cleaned}")
    }
}

fn strip_os_error_suffix(text: &str) -> &str {
    let Some(index) = text.rfind(" (os error ") else {
        return text;
    };
    if text[index..].ends_with(')') {
        text[..index].trim_end()
    } else {
        text
    }
}
