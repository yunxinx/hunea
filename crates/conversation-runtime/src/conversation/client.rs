use super::{
    ConversationRequest, TurnExecutionError, response::ConversationResponse,
    turn::run_conversation_turn_with_cancellation_and_token_progress,
};
use tool_runtime::ToolExecutorRegistry;

/// `run_conversation_turn_with_cancellation` 执行带工具回灌的对话循环。
pub async fn run_conversation_turn_with_cancellation(
    lease: &crate::ProviderClientLease,
    request: &ConversationRequest,
    executor: ToolExecutorRegistry,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<ConversationResponse, TurnExecutionError> {
    let completion = run_conversation_turn_with_cancellation_and_token_progress(
        lease,
        request,
        executor,
        cancellation,
        None,
        None,
        |_| {},
    )
    .await?;
    Ok(completion.into_response())
}
