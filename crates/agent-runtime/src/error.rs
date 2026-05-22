use mo_ai_core::ProviderError;
use thiserror::Error;

/// `AgentRuntimeError` describes failures in Lumos-owned agent orchestration.
#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("{0}")]
    Provider(#[from] ProviderError),
    #[error("agent turn cancelled")]
    Cancelled,
    #[error("agent request received no messages")]
    EmptyPrompt,
    #[error("agent reached the configured tool turn limit ({max_turns})")]
    ToolTurnLimit { max_turns: usize },
}
