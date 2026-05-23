use std::fmt;

use crate::ProviderRequestError;

/// `TurnExecutionError` 描述单轮对话执行失败。
#[derive(Debug)]
pub enum TurnExecutionError {
    Llm(ProviderRequestError),
    Cancelled,
}

impl fmt::Display for TurnExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Llm(source) => write!(f, "{source}"),
            Self::Cancelled => write!(f, "conversation turn cancelled"),
        }
    }
}

impl std::error::Error for TurnExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Llm(source) => Some(source),
            Self::Cancelled => None,
        }
    }
}

impl From<ProviderRequestError> for TurnExecutionError {
    fn from(source: ProviderRequestError) -> Self {
        match source {
            ProviderRequestError::Cancelled => Self::Cancelled,
            error => Self::Llm(error),
        }
    }
}
