use std::fmt;

use crate::runtime::native::NativeChatError;

/// `NativeAgentError` 描述内置 native agent 单轮执行失败。
#[derive(Debug)]
pub enum NativeAgentError {
    ToolsRequireExecutor,
    Chat(NativeChatError),
    Cancelled,
}

impl fmt::Display for NativeAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolsRequireExecutor => {
                write!(f, "native agent tools require an executor")
            }
            Self::Chat(source) => write!(f, "{source}"),
            Self::Cancelled => write!(f, "agent turn cancelled"),
        }
    }
}

impl std::error::Error for NativeAgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Chat(source) => Some(source),
            Self::ToolsRequireExecutor | Self::Cancelled => None,
        }
    }
}

impl From<NativeChatError> for NativeAgentError {
    fn from(source: NativeChatError) -> Self {
        match source {
            NativeChatError::Cancelled => Self::Cancelled,
            error => Self::Chat(error),
        }
    }
}
