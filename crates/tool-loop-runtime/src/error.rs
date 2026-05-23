use provider_protocol::ProviderError;
use thiserror::Error;

/// `ToolLoopError` 描述 provider turn 与工具编排阶段的失败。
#[derive(Debug, Error)]
pub enum ToolLoopError {
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("tool loop cancelled")]
    Cancelled,
    #[error("request received no messages")]
    EmptyPrompt,
    #[error("tool turn limit reached ({max_turns})")]
    ToolTurnLimit { max_turns: usize },
}
