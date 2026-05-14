use std::fmt;

use crate::NativeLlmError;

/// `NativeAgentError` 描述内置 native agent 单轮执行失败。
#[derive(Debug)]
pub enum NativeAgentError {
    Llm(NativeLlmError),
    Cancelled,
    MissingToolCallCapture,
    ToolLoopLimitExceeded { max_tool_rounds: usize },
}

impl fmt::Display for NativeAgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(source) => write!(f, "{source}"),
            Self::Cancelled => write!(f, "agent turn cancelled"),
            Self::MissingToolCallCapture => write!(f, "agent tool call capture missing"),
            Self::ToolLoopLimitExceeded { max_tool_rounds } => {
                write!(f, "agent exceeded maximum tool rounds ({max_tool_rounds})")
            }
        }
    }
}

impl std::error::Error for NativeAgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Llm(source) => Some(source),
            Self::Cancelled | Self::MissingToolCallCapture | Self::ToolLoopLimitExceeded { .. } => {
                None
            }
        }
    }
}

impl From<NativeLlmError> for NativeAgentError {
    fn from(source: NativeLlmError) -> Self {
        match source {
            NativeLlmError::Cancelled => Self::Cancelled,
            error => Self::Llm(error),
        }
    }
}
